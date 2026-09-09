mod memory;

#[cfg(feature = "sqlite")]
pub mod sqlite;

use std::{fmt, future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    DateTime(DateTime<Utc>),
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
}

pub type Rows = Vec<Row>;

#[derive(Debug)]
pub enum StoreError {
    Sql(String),
    Value(String),
    Poison(String),
    Channel(String),
    Io(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(msg)
            | Self::Value(msg)
            | Self::Poison(msg)
            | Self::Channel(msg)
            | Self::Io(msg) => write!(f, "{msg}"),
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
    ) -> BoxFuture<'a, Result<Rows, StoreError>>;

    fn last_id<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>>;
}

pub fn memory() -> Arc<dyn Store> {
    Arc::new(memory::Memory::default())
}
