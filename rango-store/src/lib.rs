pub mod spec;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "postgres")]
pub mod postgres;
mod engine;

pub use spec::{
    Action, Check, Field, Filter, Key, Link, Mass, Name, Only, Op, Order, Page, Pick, Query, Rule,
    Run, Schema, Sort, Table, Tree, Type, many,
};

use std::{fmt, future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    DateTime(DateTime<Utc>),
    Decimal(Decimal),
}

impl Serialize for Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::Float(value) => serializer.serialize_f64(*value),
            Self::Str(value) => serializer.serialize_str(value),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::DateTime(at) => serializer.serialize_str(&at.to_rfc3339()),
            Self::Decimal(value) => serializer.serialize_str(&value.to_string()),
        }
    }
}

impl Value {
    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    pub fn int(v: i64) -> Self {
        Self::Int(v)
    }

    pub fn float(v: f64) -> Self {
        Self::Float(v)
    }

    pub fn bool(v: bool) -> Self {
        Self::Bool(v)
    }

    pub fn datetime(at: DateTime<Utc>) -> Self {
        Self::DateTime(at)
    }

    pub fn decimal(v: Decimal) -> Self {
        Self::Decimal(v)
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::Str(value.clone())
    }
}

impl From<&Option<String>> for Value {
    fn from(value: &Option<String>) -> Self {
        match value {
            Some(value) => Self::Str(value.clone()),
            None => Self::Null,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum ColumnKind {
    Integer,
    Real,
    Text,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Column {
    pub name: String,
    pub kind: ColumnKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    pub values: Vec<Value>,
}

impl Row {
    pub fn get(&self, i: usize) -> Option<&Value> {
        self.values.get(i)
    }

    pub fn int(&self, i: usize) -> Result<i64, StoreError> {
        match self.values.get(i) {
            Some(Value::Int(value)) => Ok(*value),
            _ => Err(StoreError::Value(format!("row column {i} not int"))),
        }
    }

    pub fn float(&self, i: usize) -> Result<f64, StoreError> {
        match self.values.get(i) {
            Some(Value::Float(value)) => Ok(*value),
            _ => Err(StoreError::Value(format!("row column {i} not float"))),
        }
    }

    pub fn str(&self, i: usize) -> Result<String, StoreError> {
        match self.values.get(i) {
            Some(Value::Str(value)) => Ok(value.clone()),
            _ => Err(StoreError::Value(format!("row column {i} not str"))),
        }
    }

    pub fn opt_str(&self, i: usize) -> Option<String> {
        match self.values.get(i) {
            Some(Value::Str(value)) => Some(value.clone()),
            _ => None,
        }
    }

    pub fn bool(&self, i: usize) -> Result<bool, StoreError> {
        match self.values.get(i) {
            Some(Value::Bool(value)) => Ok(*value),
            Some(Value::Int(value)) => Ok(*value != 0),
            _ => Err(StoreError::Value(format!("row column {i} not bool"))),
        }
    }

    pub fn datetime(&self, i: usize) -> Result<DateTime<Utc>, StoreError> {
        let bad = || StoreError::Value(format!("row column {i} not datetime"));
        match self.values.get(i) {
            Some(Value::DateTime(at)) => Ok(*at),
            Some(Value::Int(stamp)) => DateTime::from_timestamp(*stamp, 0).ok_or_else(bad),
            Some(Value::Str(text)) => text.parse::<DateTime<Utc>>().map_err(|_| bad()),
            _ => Err(bad()),
        }
    }

    pub fn decimal(&self, i: usize) -> Result<Decimal, StoreError> {
        let bad = || StoreError::Value(format!("row column {i} not decimal"));
        match self.values.get(i) {
            Some(Value::Decimal(value)) => Ok(*value),
            Some(Value::Str(text)) => text.parse::<Decimal>().map_err(|_| bad()),
            _ => Err(bad()),
        }
    }
}

pub type Rows = Vec<Row>;

#[derive(Debug)]
pub enum StoreError {
    Sql(String),
    Value(String),
    Channel(String),
    Io(String),
    Unsupported(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(msg)
            | Self::Value(msg)
            | Self::Channel(msg)
            | Self::Io(msg)
            | Self::Unsupported(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Store: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    fn fetch<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
        kinds: &'a [ColumnKind],
    ) -> BoxFuture<'a, Result<Rows, StoreError>>;

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
        let _ = table;
        Box::pin(async { Err(StoreError::Unsupported("columns".into())) })
    }

    fn scan_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Rows, StoreError>>;

    fn total_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>>;

    fn create<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<Vec<Key>, StoreError>>;

    fn replace<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
        cells: &'a [(Name, Value)],
    ) -> BoxFuture<'a, Result<(), StoreError>>;

    fn upsert<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    fn remove<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<(), StoreError>>;

    fn evolve<'a>(
        &'a self,
        schema: &'a Schema,
        drop: bool,
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    fn mass<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Value, StoreError>>;

    fn last_id<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>>;

    fn insert<'a>(
        &'a self,
        table: &'a str,
        columns: &'a [String],
        values: &'a [Value],
    ) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move {
            let cols = columns
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let marks = vec!["?"; columns.len()].join(", ");
            let sql = format!("INSERT INTO \"{table}\" ({cols}) VALUES ({marks})");
            self.execute(&sql, values).await?;
            self.last_id(table).await
        })
    }

    fn deal<'a>(&'a self) -> BoxFuture<'a, Result<Arc<dyn Store>, StoreError>> {
        Box::pin(async { Err(StoreError::Unsupported("deal".into())) })
    }

    fn settle(self: Arc<Self>, commit: bool) -> BoxFuture<'static, Result<(), StoreError>> {
        Box::pin(async move {
            let _ = commit;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt() {
        let row = Row {
            values: vec![Value::Str("a".into()), Value::Null, Value::Int(3)],
        };
        assert_eq!(row.opt_str(0), Some("a".into()));
        assert_eq!(row.opt_str(1), None);
        assert_eq!(row.opt_str(2), None);
        assert_eq!(row.opt_str(9), None);
        assert_eq!(Value::from(&"a".to_string()), Value::Str("a".into()));
        assert_eq!(Value::from(&Some("a".to_string())), Value::Str("a".into()));
        let none: Option<String> = None;
        assert_eq!(Value::from(&none), Value::Null);
    }

    #[test]
    fn json() {
        assert_eq!(serde_json::to_string(&Value::Null).unwrap(), "null");
        assert_eq!(serde_json::to_string(&Value::Int(12)).unwrap(), "12");
        assert_eq!(serde_json::to_string(&Value::Float(1.5)).unwrap(), "1.5");
        assert_eq!(serde_json::to_string(&Value::Str("hi".into())).unwrap(), "\"hi\"");
        assert_eq!(serde_json::to_string(&Value::Bool(true)).unwrap(), "true");
        let at = DateTime::from_timestamp(0, 0).unwrap();
        assert_eq!(
            serde_json::to_string(&Value::datetime(at)).unwrap(),
            "\"1970-01-01T00:00:00+00:00\""
        );
        let money = Decimal::from_str_exact("1.50").unwrap();
        assert_eq!(
            serde_json::to_string(&Value::decimal(money)).unwrap(),
            "\"1.50\""
        );
        assert_eq!(
            serde_json::to_string(&ColumnKind::Integer).unwrap(),
            "\"Integer\""
        );
        let column = Column {
            name: "posts".into(),
            kind: ColumnKind::Text,
        };
        assert_eq!(
            serde_json::to_string(&column).unwrap(),
            "{\"name\":\"posts\",\"kind\":\"Text\"}"
        );
        let row = Row {
            values: vec![Value::Int(1), Value::Null],
        };
        assert_eq!(serde_json::to_string(&row).unwrap(), "{\"values\":[1,null]}");
    }
}
