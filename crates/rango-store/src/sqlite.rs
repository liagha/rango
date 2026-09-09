use std::{path::Path, sync::Arc};

use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, QueryResult, Statement,
    Value as SeaValue,
};

use crate::{BoxFuture, ColumnKind, Row, Rows, Store, StoreError, Value};

pub struct Sqlite {
    conn: DatabaseConnection,
}

pub async fn open(path: impl AsRef<Path>) -> Result<Arc<dyn Store>, StoreError> {
    let url = format!("sqlite://{}?mode=rwc", path.as_ref().display());
    let conn = Database::connect(&url).await.map_err(sql_err)?;
    Ok(Arc::new(Sqlite { conn }))
}

fn sql_err(err: DbErr) -> StoreError {
    StoreError::Sql(err.to_string())
}

fn bind(values: &[Value]) -> Vec<SeaValue> {
    values
        .iter()
        .map(|value| match value {
            Value::Null => SeaValue::String(None),
            Value::Int(value) => SeaValue::BigInt(Some(*value)),
            Value::Float(value) => SeaValue::Double(Some(*value)),
            Value::Str(value) => SeaValue::String(Some(Box::new(value.clone()))),
            Value::Bool(value) => SeaValue::Bool(Some(*value)),
            Value::DateTime(at) => SeaValue::BigInt(Some(at.timestamp())),
        })
        .collect()
}

fn read(row: &QueryResult, kinds: &[ColumnKind]) -> Result<Row, StoreError> {
    let mut values = Vec::with_capacity(kinds.len());
    for (i, kind) in kinds.iter().enumerate() {
        let value = match kind {
            ColumnKind::Integer => match row.try_get_by_index::<Option<i64>>(i) {
                Ok(Some(value)) => Value::Int(value),
                Ok(None) => Value::Null,
                Err(fail) => return Err(sql_err(fail)),
            },
            ColumnKind::Real => match row.try_get_by_index::<Option<f64>>(i) {
                Ok(Some(value)) => Value::Float(value),
                Ok(None) => Value::Null,
                Err(fail) => return Err(sql_err(fail)),
            },
            ColumnKind::Text => match row.try_get_by_index::<Option<String>>(i) {
                Ok(Some(value)) => Value::Str(value),
                Ok(None) => Value::Null,
                Err(fail) => return Err(sql_err(fail)),
            },
        };
        values.push(value);
    }
    Ok(Row { values })
}

fn statement(sql: &str, params: &[Value]) -> Statement {
    Statement::from_sql_and_values(DbBackend::Sqlite, sql, bind(params))
}

impl Store for Sqlite {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        Box::pin(async move {
            self.conn
                .execute(statement(&sql, &params))
                .await
                .map(|done| done.rows_affected() as usize)
                .map_err(sql_err)
        })
    }

    fn fetch<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
        kinds: &'a [ColumnKind],
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        let kinds = kinds.to_vec();
        Box::pin(async move {
            let rows = self
                .conn
                .query_all(statement(&sql, &params))
                .await
                .map_err(sql_err)?;
            rows.iter().map(|row| read(row, &kinds)).collect()
        })
    }

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<String>, StoreError>> {
        let table = table.to_string();
        Box::pin(async move {
            let rows = self
                .conn
                .query_all(Statement::from_string(
                    DbBackend::Sqlite,
                    format!("PRAGMA table_info(\"{table}\")"),
                ))
                .await
                .map_err(sql_err)?;
            let mut out = Vec::new();
            for row in &rows {
                match row.try_get_by_index::<Option<String>>(1) {
                    Ok(Some(name)) => out.push(name),
                    Ok(None) => {}
                    Err(fail) => return Err(sql_err(fail)),
                }
            }
            Ok(out)
        })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move {
            let rows = self
                .conn
                .query_all(Statement::from_string(
                    DbBackend::Sqlite,
                    "SELECT last_insert_rowid()".to_string(),
                ))
                .await
                .map_err(sql_err)?;
            match rows
                .first()
                .and_then(|row| row.try_get_by_index::<Option<i64>>(0).ok().flatten())
            {
                Some(id) => Ok(id),
                None => Err(StoreError::Value("no last id".into())),
            }
        })
    }
}
