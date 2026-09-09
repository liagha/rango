use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::{
    Connection, params_from_iter,
    types::{Value as SqValue, ValueRef},
};

use crate::{BoxFuture, Row, Rows, Store, StoreError, Value};

pub struct Sqlite {
    conn: Arc<Mutex<Connection>>,
}

pub fn open(path: impl AsRef<Path>) -> Result<Arc<dyn Store>, StoreError> {
    let conn = Connection::open(path).map_err(|err| StoreError::Io(err.to_string()))?;
    Ok(Arc::new(Sqlite {
        conn: Arc::new(Mutex::new(conn)),
    }))
}

fn sql_err(err: rusqlite::Error) -> StoreError {
    StoreError::Sql(err.to_string())
}

fn poison() -> StoreError {
    StoreError::Poison("sqlite lock".into())
}

fn channel(err: tokio::task::JoinError) -> StoreError {
    StoreError::Channel(err.to_string())
}

fn bind(values: &[Value]) -> Vec<SqValue> {
    values
        .iter()
        .map(|value| match value {
            Value::Null => SqValue::Null,
            Value::Int(value) => SqValue::Integer(*value),
            Value::Float(value) => SqValue::Real(*value),
            Value::Str(value) => SqValue::Text(value.clone()),
            Value::Bool(value) => SqValue::Integer(if *value { 1 } else { 0 }),
            Value::DateTime(at) => SqValue::Integer(at.timestamp()),
        })
        .collect()
}

fn read(row: &rusqlite::Row<'_>, cols: usize) -> Result<Row, StoreError> {
    let mut values = Vec::with_capacity(cols);
    for i in 0..cols {
        values.push(match row.get_ref(i).map_err(sql_err)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(value) => Value::Int(value),
            ValueRef::Real(value) => Value::Float(value),
            ValueRef::Text(value) => Value::Str(String::from_utf8_lossy(value).into_owned()),
            ValueRef::Blob(_) => return Err(StoreError::Value("blob unsupported".into())),
        });
    }
    Ok(Row { values })
}

impl Store for Sqlite {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let conn = self.conn.clone();
        let sql = sql.to_string();
        let values = bind(params);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let conn = conn.lock().map_err(|_| poison())?;
                conn.execute(&sql, params_from_iter(values.iter()))
                    .map_err(sql_err)
            })
            .await
            .map_err(channel)?
        })
    }

    fn fetch<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let conn = self.conn.clone();
        let sql = sql.to_string();
        let values = bind(params);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let conn = conn.lock().map_err(|_| poison())?;
                let mut stmt = conn.prepare(&sql).map_err(sql_err)?;
                let cols = stmt.column_count();
                let mut iter = stmt
                    .query(params_from_iter(values.iter()))
                    .map_err(sql_err)?;
                let mut rows = Vec::new();
                while let Some(row) = iter.next().map_err(sql_err)? {
                    rows.push(read(row, cols)?);
                }
                Ok(rows)
            })
            .await
            .map_err(channel)?
        })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        let conn = self.conn.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                conn.lock()
                    .map_err(|_| poison())
                    .map(|c| c.last_insert_rowid())
            })
            .await
            .map_err(channel)?
        })
    }
}
