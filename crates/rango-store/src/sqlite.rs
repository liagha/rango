use std::{path::Path, sync::Arc};

use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, QueryResult, Statement,
    Value as SeaValue,
};

use crate::{BoxFuture, Column, ColumnKind, Row, Rows, Store, StoreError, Value};

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

fn affinity_of(sql: &str) -> ColumnKind {
    let sql = sql.to_uppercase();
    if sql.contains("INT") {
        ColumnKind::Integer
    } else if sql.contains("CHAR") || sql.contains("CLOB") || sql.contains("TEXT") {
        ColumnKind::Text
    } else if sql.contains("REAL") || sql.contains("FLOA") || sql.contains("DOUB") {
        ColumnKind::Real
    } else {
        ColumnKind::Text
    }
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

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
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
                let name = match row.try_get_by_index::<Option<String>>(1) {
                    Ok(name) => name.unwrap_or_default(),
                    Err(fail) => return Err(sql_err(fail)),
                };
                let sql = match row.try_get_by_index::<Option<String>>(2) {
                    Ok(sql) => sql.unwrap_or_default(),
                    Err(fail) => return Err(sql_err(fail)),
                };
                out.push(Column {
                    name,
                    kind: affinity_of(&sql),
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> Arc<dyn Store> {
        let path = std::env::temp_dir().join(format!("rango-test-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        open(&path).await.unwrap()
    }

    #[tokio::test]
    async fn roundtrip() {
        let db = store().await;
        db.execute(
            "CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, age INTEGER, score REAL, flag INTEGER, at INTEGER)",
            &[],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO t (name, age, score, flag, at) VALUES (?, ?, ?, ?, ?)",
            &[
                Value::str("a"),
                Value::int(1),
                Value::float(1.5),
                Value::bool(true),
                Value::int(1700000001),
            ],
        )
        .await
        .unwrap();
        assert_eq!(db.last_id("t").await.unwrap(), 1);
        let kinds = [
            ColumnKind::Integer,
            ColumnKind::Text,
            ColumnKind::Integer,
            ColumnKind::Real,
            ColumnKind::Integer,
            ColumnKind::Integer,
        ];
        let rows = db
            .fetch("SELECT * FROM t ORDER BY id", &[], &kinds)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].str(1).unwrap(), "a");
        assert_eq!(rows[0].int(2).unwrap(), 1);
        assert_eq!(rows[0].float(3).unwrap(), 1.5);
        assert!(rows[0].bool(4).unwrap());
        assert_eq!(rows[0].datetime(5).unwrap().timestamp(), 1700000001);
        let cols = db.columns("t").await.unwrap();
        assert_eq!(
            cols.iter().map(|col| col.name.as_str()).collect::<Vec<_>>(),
            vec!["id", "name", "age", "score", "flag", "at"]
        );
        let count = db
            .fetch("SELECT COUNT(*) FROM t", &[], &[ColumnKind::Integer])
            .await
            .unwrap();
        assert_eq!(count[0].int(0).unwrap(), 1);
        let touched = db
            .execute(
                "UPDATE t SET age = ? WHERE id = ?",
                &[Value::int(2), Value::int(1)],
            )
            .await
            .unwrap();
        assert_eq!(touched, 1);
        let gone = db
            .execute("DELETE FROM t WHERE id = ?", &[Value::int(1)])
            .await
            .unwrap();
        assert_eq!(gone, 1);
    }

    #[tokio::test]
    async fn missing() {
        let db = store().await;
        assert!(db.columns("nope").await.unwrap().is_empty());
        assert!(db.fetch("SELECT * FROM nope", &[], &[]).await.is_err());
    }
}
