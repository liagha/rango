//! Typed, transaction-safe storage for Rango over SQLite and PostgreSQL.
//!
//! The [`Store`] trait is the database face: typed [`Value`]s in, typed [`Row`]s
//! out. Transactions come from [`Store::deal`] and [`Store::settle`]. A
//! [`Schema`] describes a table; the `spec` module defines its queries and
//! field rules. SQLite is the default backend; enable the `postgres` feature
//! for PostgreSQL.
#![warn(missing_docs)]

mod engine;
/// PostgreSQL backend behind the `postgres` feature.
#[cfg(feature = "postgres")]
pub mod postgres;
/// Query and schema specification types shared by all backends.
pub mod spec;
/// SQLite backend, enabled by default.
#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use spec::{
    Action, Choice, Field, Filter, Key, Link, Mass, Name, Only, Op, Order, Page, Policy, Query, Rule,
    Run, Schema, Sort, Table, Tree,
};

/// SQLite backend type, enabled by default.
#[cfg(feature = "sqlite")]
pub use sqlite::Sqlite;
/// PostgreSQL backend type behind the `postgres` feature.
#[cfg(feature = "postgres")]
pub use postgres::Postgres;

use std::{fmt, future::Future, hash::{Hash, Hasher}, pin::Pin, str::FromStr, sync::Arc};

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use rust_decimal::Decimal;
use sea_orm::DbErr;
use serde::Serialize;

/// A single typed database value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// SQL NULL.
    Null,
    /// Signed 64-bit integer.
    Int(i64),
    /// Double-precision float.
    Float(f64),
    /// UTF-8 string.
    Str(String),
    /// Boolean.
    Bool(bool),
    /// UTC timestamp.
    DateTime(DateTime<Utc>),
    /// Exact decimal.
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

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Null => {}
            Self::Int(v) => v.hash(state),
            Self::Float(v) => v.to_bits().hash(state),
            Self::Str(v) => v.hash(state),
            Self::Bool(v) => v.hash(state),
            Self::DateTime(at) => at.timestamp().hash(state),
            Self::Decimal(v) => v.to_string().hash(state),
        }
    }
}

impl Value {
    /// Shorthand for [`Value::Str`] from anything `Into<String>`.
    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    /// Shorthand for [`Value::Int`].
    pub fn int(v: i64) -> Self {
        Self::Int(v)
    }

    /// Shorthand for [`Value::Float`].
    pub fn float(v: f64) -> Self {
        Self::Float(v)
    }

    /// Shorthand for [`Value::Bool`].
    pub fn bool(v: bool) -> Self {
        Self::Bool(v)
    }

    /// Shorthand for [`Value::DateTime`].
    pub fn datetime(at: DateTime<Utc>) -> Self {
        Self::DateTime(at)
    }

    /// Shorthand for [`Value::Decimal`].
    pub fn decimal(v: Decimal) -> Self {
        Self::Decimal(v)
    }

    /// This value as a SQL literal for `DEFAULT` columns.
    pub fn literal(&self) -> String {
        match self {
            Self::Null => "NULL".into(),
            Self::Int(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::Str(v) => format!("'{v}'"),
            Self::Bool(v) => if *v { "1".into() } else { "0".into() },
            Self::DateTime(at) => at.timestamp().to_string(),
            Self::Decimal(v) => format!("'{v}'"),
        }
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

/// How a column's cells map to [`Value`]s when read back.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Column {
    /// Integer affinity ([`Value::Int`]).
    Integer,
    /// Floating-point affinity ([`Value::Float`]).
    Real,
    /// Text affinity ([`Value::Str`]).
    Text,
}

impl Column {
    /// Affinity of a field's SQL type name.
    pub fn of(raw: &str) -> Self {
        let sql = raw.to_uppercase();
        if sql.contains("INT") || sql.contains("BOOL") {
            Column::Integer
        } else if sql.contains("CHAR") || sql.contains("TEXT") {
            Column::Text
        } else if sql.contains("REAL") || sql.contains("FLOA") || sql.contains("DOUB") {
            Column::Real
        } else {
            Column::Text
        }
    }
}

/// One fetched row, cells aligned with the query's columns.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Row {
    /// Cell values, one per selected column.
    pub values: Vec<Value>,
}

impl Row {
    /// Cell at index `i`, if present.
    pub fn get(&self, i: usize) -> Option<&Value> {
        self.values.get(i)
    }

    /// Cell `i` as `i64`, or [`StoreError::Value`].
    pub fn int(&self, i: usize) -> Result<i64, StoreError> {
        match self.values.get(i) {
            Some(Value::Int(value)) => Ok(*value),
            _ => Err(StoreError::Value(format!("row column {i} not int"))),
        }
    }

    /// Cell `i` as `f64`, or [`StoreError::Value`].
    pub fn float(&self, i: usize) -> Result<f64, StoreError> {
        match self.values.get(i) {
            Some(Value::Float(value)) => Ok(*value),
            _ => Err(StoreError::Value(format!("row column {i} not float"))),
        }
    }

    /// Cell `i` as `String`, or [`StoreError::Value`].
    pub fn str(&self, i: usize) -> Result<String, StoreError> {
        match self.values.get(i) {
            Some(Value::Str(value)) => Ok(value.clone()),
            _ => Err(StoreError::Value(format!("row column {i} not str"))),
        }
    }

    /// Cell `i` as `Option<String>`, `None` when null or not text.
    pub fn opt_str(&self, i: usize) -> Option<String> {
        match self.values.get(i) {
            Some(Value::Str(value)) => Some(value.clone()),
            _ => None,
        }
    }

    /// Cell `i` as `bool` (integers count as truthy), or [`StoreError::Value`].
    pub fn bool(&self, i: usize) -> Result<bool, StoreError> {
        match self.values.get(i) {
            Some(Value::Bool(value)) => Ok(*value),
            Some(Value::Int(value)) => Ok(*value != 0),
            _ => Err(StoreError::Value(format!("row column {i} not bool"))),
        }
    }

    /// Cell `i` as `DateTime<Utc>` (accepts int timestamps and RFC 3339 text), or [`StoreError::Value`].
    pub fn datetime(&self, i: usize) -> Result<DateTime<Utc>, StoreError> {
        let bad = || StoreError::Value(format!("row column {i} not datetime"));
        match self.values.get(i) {
            Some(Value::DateTime(at)) => Ok(*at),
            Some(Value::Int(stamp)) => DateTime::from_timestamp(*stamp, 0).ok_or_else(bad),
            Some(Value::Str(text)) => text.parse::<DateTime<Utc>>().map_err(|_| bad()),
            _ => Err(bad()),
        }
    }

    /// Cell `i` as `Decimal` (accepts text), or [`StoreError::Value`].
    pub fn decimal(&self, i: usize) -> Result<Decimal, StoreError> {
        let bad = || StoreError::Value(format!("row column {i} not decimal"));
        match self.values.get(i) {
            Some(Value::Decimal(value)) => Ok(*value),
            Some(Value::Str(text)) => text.parse::<Decimal>().map_err(|_| bad()),
            _ => Err(bad()),
        }
    }

    /// A [`Reader`] over the row's cells.
    pub fn cells(&self) -> Cells<'_> {
        Cells::new(&self.values)
    }
}

/// A batch of fetched rows.
pub type Rows = Vec<Row>;

/// Errors surfaced by the store, backends, and value conversion.
#[derive(Debug)]
pub enum StoreError {
    /// SQL or driver error.
    Sql(String),
    /// Invalid value, column, or key conversion.
    Value(String),
    /// Streaming or channel error.
    Channel(String),
    /// I/O error.
    Io(String),
    /// Violation of a referenced-row constraint.
    Reference(String),
    /// Operation not supported by the backend.
    Unsupported(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(msg)
            | Self::Value(msg)
            | Self::Channel(msg)
            | Self::Io(msg)
            | Self::Reference(msg)
            | Self::Unsupported(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<DbErr> for StoreError {
    fn from(err: DbErr) -> Self {
        let msg = err.to_string();
        if msg.contains("(code: 787)")
            || msg.contains("FOREIGN KEY constraint failed")
            || msg.contains("23503")
        {
            Self::Reference(msg)
        } else {
            Self::Sql(msg)
        }
    }
}

/// UI hint for how a field is rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Widget {
    /// Plain text input.
    Text,
    /// Integer input.
    Int,
    /// Float input.
    Flt,
    /// Money input.
    Money,
    /// Date input.
    Date,
    /// Checkbox.
    Check,
    /// Select from a fixed set of choices.
    Choice,
}

/// Sink for typed cell values during serialization.
pub trait Writer {
    /// Writes a null cell.
    fn nothing(&mut self);
    /// Writes an integer cell.
    fn int(&mut self, value: i64);
    /// Writes a float cell.
    fn flt(&mut self, value: f64);
    /// Writes a string cell.
    fn str(&mut self, value: &str);
}

/// Source of typed cell values during deserialization.
pub trait Reader {
    /// Reads a null cell; `true` when the next cell is null.
    fn nothing(&mut self) -> bool;
    /// Reads the next cell as `i64`, or [`StoreError::Value`].
    fn int(&mut self) -> Result<i64, StoreError>;
    /// Reads the next cell as `f64`, or [`StoreError::Value`].
    fn flt(&mut self) -> Result<f64, StoreError>;
    /// Reads the next cell as `String`, or [`StoreError::Value`].
    fn str(&mut self) -> Result<String, StoreError>;
}

/// A type that can be stored in a single column.
pub trait Storable: Send + Sync + 'static {
    /// Serializes `self` into a [`Writer`].
    fn put(&self, w: &mut dyn Writer);
    /// Deserializes a `Self` from a [`Reader`].
    fn take(r: &mut dyn Reader) -> Result<Self, StoreError>
    where
        Self: Sized;
    /// SQL column type for this value.
    fn dtype() -> &'static str;
}

/// Human presentation of a stored type: widget, parsing, and text.
pub trait Show: Sized {
    /// Input widget for this type.
    fn widget() -> Widget;
    /// Parses raw user input into a `Self`.
    fn parse(raw: &str) -> Result<Self, StoreError>;
    /// Renders this value as display text.
    fn text(&self) -> String;
}

macro_rules! cells {
    ($($cell:ty),*) => {$(
        impl Storable for $cell {
            fn put(&self, w: &mut dyn Writer) {
                w.int(*self as i64);
            }

            fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
                Ok(r.int()? as $cell)
            }

            fn dtype() -> &'static str {
                "INTEGER"
            }
        }

        impl Show for $cell {
            fn widget() -> Widget {
                Widget::Int
            }

            fn parse(raw: &str) -> Result<Self, StoreError> {
                raw.parse::<$cell>()
                    .map_err(|_| StoreError::Value(format!("bad int {raw}")))
            }

            fn text(&self) -> String {
                self.to_string()
            }
        }
    )*};
}

cells!(i64, i32, i16, i8, u64, u32, u16, u8, isize, usize);

impl Storable for String {
    fn put(&self, w: &mut dyn Writer) {
        w.str(self);
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        r.str()
    }

    fn dtype() -> &'static str {
        "TEXT"
    }
}

impl Show for String {
    fn widget() -> Widget {
        Widget::Text
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        Ok(raw.into())
    }

    fn text(&self) -> String {
        self.clone()
    }
}

impl Storable for f64 {
    fn put(&self, w: &mut dyn Writer) {
        w.flt(*self);
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        r.flt()
    }

    fn dtype() -> &'static str {
        "REAL"
    }
}

impl Show for f64 {
    fn widget() -> Widget {
        Widget::Flt
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        raw.parse::<f64>()
            .map_err(|_| StoreError::Value(format!("bad float {raw}")))
    }

    fn text(&self) -> String {
        self.to_string()
    }
}

impl Storable for bool {
    fn put(&self, w: &mut dyn Writer) {
        w.int(if *self { 1 } else { 0 });
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        Ok(r.int()? != 0)
    }

    fn dtype() -> &'static str {
        "INTEGER"
    }
}

impl Show for bool {
    fn widget() -> Widget {
        Widget::Check
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        match raw {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" | "" => Ok(false),
            _ => Err(StoreError::Value(format!("bad bool {raw}"))),
        }
    }

    fn text(&self) -> String {
        self.to_string()
    }
}

impl Storable for Decimal {
    fn put(&self, w: &mut dyn Writer) {
        w.str(&self.to_string());
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        Decimal::from_str(&r.str()?).map_err(|_| StoreError::Value("bad decimal".into()))
    }

    fn dtype() -> &'static str {
        "TEXT"
    }
}

impl Show for Decimal {
    fn widget() -> Widget {
        Widget::Money
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        Decimal::from_str(raw).map_err(|_| StoreError::Value(format!("bad decimal {raw}")))
    }

    fn text(&self) -> String {
        self.to_string()
    }
}

impl Storable for DateTime<Utc> {
    fn put(&self, w: &mut dyn Writer) {
        w.int(self.timestamp());
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        DateTime::from_timestamp(r.int()?, 0)
            .ok_or_else(|| StoreError::Value("bad datetime".into()))
    }

    fn dtype() -> &'static str {
        "INTEGER"
    }
}

impl Show for DateTime<Utc> {
    fn widget() -> Widget {
        Widget::Date
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        let bad = || StoreError::Value(format!("bad date {raw}"));
        if let Ok(at) = DateTime::parse_from_rfc3339(raw) {
            return Ok(at.with_timezone(&Utc));
        }
        if let Ok(at) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M") {
            return Ok(at.and_utc());
        }
        NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map(|day| day.and_time(NaiveTime::MIN).and_utc())
            .map_err(|_| bad())
    }

    fn text(&self) -> String {
        self.format("%Y-%m-%d %H:%M").to_string()
    }
}

impl Storable for NaiveDate {
    fn put(&self, w: &mut dyn Writer) {
        w.str(&self.format("%Y-%m-%d").to_string());
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        NaiveDate::parse_from_str(&r.str()?, "%Y-%m-%d")
            .map_err(|_| StoreError::Value("bad date".into()))
    }

    fn dtype() -> &'static str {
        "TEXT"
    }
}

impl Show for NaiveDate {
    fn widget() -> Widget {
        Widget::Date
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map_err(|_| StoreError::Value(format!("bad date {raw}")))
    }

    fn text(&self) -> String {
        self.format("%Y-%m-%d").to_string()
    }
}

impl Storable for NaiveTime {
    fn put(&self, w: &mut dyn Writer) {
        w.str(&self.format("%H:%M:%S").to_string());
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        NaiveTime::parse_from_str(&r.str()?, "%H:%M:%S")
            .map_err(|_| StoreError::Value("bad time".into()))
    }

    fn dtype() -> &'static str {
        "TEXT"
    }
}

impl Show for NaiveTime {
    fn widget() -> Widget {
        Widget::Text
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        NaiveTime::parse_from_str(raw, "%H:%M:%S")
            .map_err(|_| StoreError::Value(format!("bad time {raw}")))
    }

    fn text(&self) -> String {
        self.format("%H:%M").to_string()
    }
}

impl<T: Storable> Storable for Option<T> {
    fn put(&self, w: &mut dyn Writer) {
        match self {
            Some(value) => value.put(w),
            None => w.nothing(),
        }
    }

    fn take(r: &mut dyn Reader) -> Result<Self, StoreError> {
        if r.nothing() {
            Ok(None)
        } else {
            Ok(Some(T::take(r)?))
        }
    }

    fn dtype() -> &'static str {
        T::dtype()
    }
}

impl Writer for Value {
    fn nothing(&mut self) {
        *self = Value::Null;
    }

    fn int(&mut self, value: i64) {
        *self = Value::int(value);
    }

    fn flt(&mut self, value: f64) {
        *self = Value::float(value);
    }

    fn str(&mut self, value: &str) {
        *self = Value::str(value);
    }
}

impl Writer for Vec<Value> {
    fn nothing(&mut self) {
        self.push(Value::Null);
    }

    fn int(&mut self, value: i64) {
        self.push(Value::int(value));
    }

    fn flt(&mut self, value: f64) {
        self.push(Value::float(value));
    }

    fn str(&mut self, value: &str) {
        self.push(Value::str(value));
    }
}

/// [`Reader`] over a slice of values; the cursor advances as cells are read.
pub struct Cells<'a> {
    values: &'a [Value],
    at: usize,
}

impl<'a> Cells<'a> {
    /// A cursor over `values` starting at the first cell.
    pub fn new(values: &'a [Value]) -> Self {
        Self { values, at: 0 }
    }
}

impl Reader for Cells<'_> {
    fn nothing(&mut self) -> bool {
        match self.values.get(self.at) {
            Some(Value::Null) => {
                self.at += 1;
                true
            }
            None => true,
            Some(_) => false,
        }
    }

    fn int(&mut self) -> Result<i64, StoreError> {
        let bad = || StoreError::Value(format!("cell {} not int", self.at));
        match self.values.get(self.at) {
            Some(Value::Int(value)) => {
                self.at += 1;
                Ok(*value)
            }
            Some(Value::Bool(value)) => {
                self.at += 1;
                Ok(*value as i64)
            }
            Some(Value::DateTime(at)) => {
                self.at += 1;
                Ok(at.timestamp())
            }
            _ => Err(bad()),
        }
    }

    fn flt(&mut self) -> Result<f64, StoreError> {
        let at = self.at;
        let bad = || StoreError::Value(format!("cell {at} not float"));
        match self.values.get(at) {
            Some(Value::Float(value)) => {
                self.at += 1;
                Ok(*value)
            }
            Some(Value::Int(value)) => {
                self.at += 1;
                Ok(*value as f64)
            }
            Some(Value::Bool(value)) => {
                self.at += 1;
                Ok(*value as i64 as f64)
            }
            Some(Value::Decimal(value)) => {
                self.at += 1;
                value.to_string().parse::<f64>().map_err(|_| bad())
            }
            _ => Err(bad()),
        }
    }

    fn str(&mut self) -> Result<String, StoreError> {
        let at = self.at;
        let bad = || StoreError::Value(format!("cell {at} not str"));
        match self.values.get(at) {
            Some(Value::Str(value)) => {
                self.at += 1;
                Ok(value.clone())
            }
            Some(Value::Decimal(value)) => {
                self.at += 1;
                Ok(value.to_string())
            }
            Some(Value::DateTime(value)) => {
                self.at += 1;
                Ok(value.to_rfc3339())
            }
            _ => Err(bad()),
        }
    }
}

/// A boxed, `Send`, `'a`-bounded future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A versioned migration file to apply.
pub struct Pending {
    /// File name, e.g. `0001_create_users.sql`.
    pub name: String,
    /// SQL body, possibly many statements.
    pub sql: String,
    /// Recording checksum of the file content.
    pub checksum: String,
}

/// The migrations ledger table: each applied file's name, checksum, and time.
pub fn ledger() -> Schema {
    Schema {
        table: Table("_migrations"),
        fields: vec![
            Field::key::<String>("name"),
            Field::str("checksum"),
            Field::str("applied_at"),
        ],
        rules: Vec::new(),
    }
}

/// Splits SQL on top-level semicolons, ignoring those inside single and double
/// quoted strings, dollar quotes, and comments.
pub(crate) fn statements(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut stmt = String::new();
    let mut at = 0;
    while at < chars.len() {
        let ch = chars[at];
        match ch {
            '\'' | '"' => {
                stmt.push(ch);
                at += 1;
                while at < chars.len() {
                    let c = chars[at];
                    if c == ch && at + 1 < chars.len() && chars[at + 1] == ch {
                        stmt.push(c);
                        stmt.push(c);
                        at += 2;
                    } else if c == ch {
                        stmt.push(c);
                        at += 1;
                        break;
                    } else {
                        stmt.push(c);
                        at += 1;
                    }
                }
            }
            '$' => {
                if let Some(end) = dollar(&chars, at) {
                    let delim: String = chars[at..=end].iter().collect();
                    stmt.push_str(&delim);
                    let mut look = at + delim.len();
                    let close = loop {
                        if look + delim.len() > chars.len() {
                            break None;
                        }
                        if chars[look..look + delim.len()].iter().collect::<String>() == delim {
                            break Some(look);
                        }
                        look += 1;
                    };
                    match close {
                        Some(look) => {
                            stmt.push_str(
                                &chars[at + delim.len()..look + delim.len()].iter().collect::<String>(),
                            );
                            at = look + delim.len();
                        }
                        None => {
                            stmt.push_str(&chars[at + delim.len()..].iter().collect::<String>());
                            at = chars.len();
                        }
                    }
                } else {
                    stmt.push('$');
                    at += 1;
                }
            }
            '-' if at + 1 < chars.len() && chars[at + 1] == '-' => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
            }
            '/' if at + 1 < chars.len() && chars[at + 1] == '*' => {
                at += 2;
                while at < chars.len() {
                    if chars[at] == '*' && at + 1 < chars.len() && chars[at + 1] == '/' {
                        at += 2;
                        break;
                    }
                    at += 1;
                }
            }
            ';' => {
                let sql = stmt.trim();
                if !sql.is_empty() {
                    out.push(sql.into());
                }
                stmt.clear();
                at += 1;
            }
            _ => {
                stmt.push(ch);
                at += 1;
            }
        }
    }
    let tail = stmt.trim();
    if !tail.is_empty() {
        out.push(tail.into());
    }
    out
}

fn dollar(chars: &[char], at: usize) -> Option<usize> {
    let head = at + 1 < chars.len() && (chars[at + 1].is_ascii_alphabetic() || chars[at + 1] == '_' || chars[at + 1] == '$');
    if !head {
        return None;
    }
    let mut j = at + 1;
    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    if j < chars.len() && chars[j] == '$' {
        Some(j)
    } else {
        None
    }
}

/// A database backend: SQL and typed schema operations over a connection.
pub trait Store: Send + Sync + 'static {
    /// Runs `sql` with `params`, returning rows affected.
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    /// Runs `sql` with `params`, decoding rows per `kinds`.
    fn fetch<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
        kinds: &'a [Column],
    ) -> BoxFuture<'a, Result<Rows, StoreError>>;

    /// Introspects the columns of `table` as `(name, affinity)` pairs.
    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<(String, Column)>, StoreError>> {
        let _ = table;
        Box::pin(async { Err(StoreError::Unsupported("columns".into())) })
    }

    /// Fetches the rows of `schema` matching `query`.
    fn scan_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Rows, StoreError>>;

    /// Counts the rows of `schema` matching `query`.
    fn total_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    /// Creates `schema`'s table if absent.
    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>>;

    /// Inserts a batch of `schema` rows, returning each new row's key.
    fn create<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<Vec<Key>, StoreError>>;

    /// Overwrites `key`'s row of `schema` with `cells`.
    fn replace<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
        cells: &'a [(Name, Value)],
    ) -> BoxFuture<'a, Result<(), StoreError>>;

    /// Inserts a `schema` batch, overwriting any matching keys, returning rows touched.
    fn upsert<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    /// Deletes `key`'s row of `schema`.
    fn remove<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<(), StoreError>>;

    /// Migrates `schema`'s table to the latest definition, returning statements run.
    fn evolve<'a>(
        &'a self,
        schema: &'a Schema,
        drop: bool,
    ) -> BoxFuture<'a, Result<usize, StoreError>>;

    /// Aggregates `schema` rows matching `query` per `query.mass`.
    fn mass<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Value, StoreError>>;

    /// Last autoincrement id inserted into `table`.
    fn last_id<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>>;

    /// Raw single-row insert into `table` returning its id.
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

    /// Persistent signing secret, generated and stored on first call.
    fn secret<'a>(&'a self) -> BoxFuture<'a, String> {
        Box::pin(async move {
            let settings = Schema {
                table: Table("setting"),
                fields: vec![Field::key::<String>("name"), Field::str("value")],
                rules: Vec::new(),
            };
            let read = || async {
                let rows = self
                    .scan_query(
                        &settings,
                        &Query {
                            tree: Tree::Leaf(Filter {
                                field: Name("name"),
                                op: Op::Eq,
                                value: Value::str("secret"),
                            }),
                            sort: Vec::new(),
                            page: Page::all(),
                            only: Only::All,
                            mass: None,
                        },
                    )
                    .await
                    .ok()?;
                match rows.first()?.get(1)? {
                    Value::Str(secret) if !secret.is_empty() => Some(secret.clone()),
                    _ => None,
                }
            };
            let fresh = || format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
            if self.define(&settings).await.is_err() {
                tracing::warn!("ephemeral secret: no setting table");
                return fresh();
            }
            if let Some(secret) = read().await {
                return secret;
            }
            let next = fresh();
            let _ = self
                .create(
                    &settings,
                    &[vec![
                        (Name("name"), Value::str("secret")),
                        (Name("value"), Value::str(next.clone())),
                    ]],
                )
                .await;
            read().await.unwrap_or_else(|| {
                tracing::warn!("ephemeral secret: no secret row");
                fresh()
            })
        })
    }

    /// Applies every unapplied [`Pending`] migration in order inside one
    /// transaction, recording each applied file's checksum. A pending file
    /// whose recorded checksum no longer matches is a hard error — applied
    /// files are immutable, repairs are forward migrations.
    fn migrate<'a>(
        &'a self,
        pending: &'a [Pending],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        Box::pin(async move {
            let ledger = ledger();
            self.define(&ledger).await?;
            let rows = self
                .scan_query(
                    &ledger,
                    &Query {
                        tree: Tree::And(Vec::new()),
                        sort: Vec::new(),
                        page: Page::all(),
                        only: Only::All,
                        mass: None,
                    },
                )
                .await?;
            let mut recorded = std::collections::HashMap::new();
            for row in rows {
                let name = row.opt_str(0).unwrap_or_default();
                let checksum = row.opt_str(1).unwrap_or_default();
                recorded.insert(name, checksum);
            }
            let mut todo = Vec::new();
            for file in pending {
                match recorded.get(&file.name) {
                    Some(checksum) if checksum == &file.checksum => {}
                    Some(_) => {
                        return Err(StoreError::Value(format!(
                            "migration {} changed after it was applied — repairs are forward migrations",
                            file.name
                        )));
                    }
                    None => todo.push(file),
                }
            }
            let deal = self.deal().await?;
            match async {
                let mut applied = 0;
                for file in &todo {
                    for sql in statements(&file.sql) {
                        deal.execute(&sql, &[]).await?;
                    }
                    deal.create(
                        &ledger,
                        &[vec![
                            (Name("name"), Value::str(&file.name)),
                            (Name("checksum"), Value::str(&file.checksum)),
                            (Name("applied_at"), Value::str(Utc::now().to_rfc3339())),
                        ]],
                    )
                    .await?;
                    applied += 1;
                }
                Ok::<usize, StoreError>(applied)
            }
            .await
            {
                Ok(applied) => {
                    deal.settle(true).await?;
                    Ok(applied)
                }
                Err(err) => {
                    let _ = deal.settle(false).await;
                    Err(err)
                }
            }
        })
    }

    /// Opens a transaction as an independent [`Store`].
    fn deal<'a>(&'a self) -> BoxFuture<'a, Result<Arc<dyn Store>, StoreError>> {
        Box::pin(async { Err(StoreError::Unsupported("deal".into())) })
    }

    /// Commits or rolls back the transaction from [`Store::deal`].
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
    fn split() {
        let sql = "-- hello; world\nCREATE TABLE t (a TEXT, b TEXT DEFAULT 'x;y'); \n\nINSERT INTO t VALUES ('a;b');\n/* block; comment */ SELECT 1;";
        assert_eq!(
            statements(sql),
            vec![
                "CREATE TABLE t (a TEXT, b TEXT DEFAULT 'x;y')".to_string(),
                "INSERT INTO t VALUES ('a;b')".to_string(),
                "SELECT 1".to_string(),
            ]
        );
        let tag = "SELECT $body$ ; $body$; SELECT 2;";
        assert_eq!(
            statements(tag),
            vec!["SELECT $body$ ; $body$".to_string(), "SELECT 2".to_string()]
        );
        assert_eq!(statements(""), Vec::<String>::new());
        assert_eq!(statements("SELECT 1"), vec!["SELECT 1".to_string()]);
    }

    #[test]
    fn json() {
        assert_eq!(serde_json::to_string(&Value::Null).unwrap(), "null");
        assert_eq!(serde_json::to_string(&Value::Int(12)).unwrap(), "12");
        assert_eq!(serde_json::to_string(&Value::Float(1.5)).unwrap(), "1.5");
        assert_eq!(
            serde_json::to_string(&Value::Str("hi".into())).unwrap(),
            "\"hi\""
        );
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
        assert_eq!(serde_json::to_string(&Column::Integer).unwrap(), "\"Integer\"");
        assert_eq!(serde_json::to_string(&Column::Text).unwrap(), "\"Text\"");
        assert_eq!(
            serde_json::to_string(&[("name", Column::Integer)]).unwrap(),
            "[[\"name\",\"Integer\"]]"
        );
        let row = Row {
            values: vec![Value::Int(1), Value::Null],
        };
        assert_eq!(
            serde_json::to_string(&row).unwrap(),
            "{\"values\":[1,null]}"
        );
    }
}
