mod memory;

#[cfg(feature = "sqlite")]
pub mod sqlite;

use std::{fmt, future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnKind {
    Integer,
    Real,
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub kind: ColumnKind,
}

#[derive(Clone, Debug)]
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
    Poison(String),
    Channel(String),
    Io(String),
    Unsupported(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(msg)
            | Self::Value(msg)
            | Self::Poison(msg)
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
}

pub fn memory() -> Arc<dyn Store> {
    Arc::new(memory::Memory::default())
}
