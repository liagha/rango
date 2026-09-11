use std::{path::Path, sync::Arc};

use chrono::{NaiveTime, TimeDelta};
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, QueryResult, Statement,
    Value as SeaValue,
};

use crate::{
    BoxFuture, Column, ColumnKind, Filter, Name, Only, Op, Order, Query, Row, Rows, Schema, Sort,
    Store, StoreError, Tree, Value, spec::affinity,
};

pub struct Sqlite {
    conn: DatabaseConnection,
}

pub async fn open(path: impl AsRef<Path>) -> Result<Arc<dyn Store>, StoreError> {
    let url = format!("sqlite://{}?mode=rwc", path.as_ref().display());
    connect(&url).await
}

pub async fn open_wal(path: impl AsRef<Path>) -> Result<Arc<dyn Store>, StoreError> {
    let url = format!("sqlite://{}?mode=rwc", path.as_ref().display());
    let conn = Database::connect(&url).await.map_err(sql_err)?;
    for pragma in [
        "PRAGMA journal_mode=WAL",
        "PRAGMA synchronous=NORMAL",
        "PRAGMA busy_timeout=5000",
    ] {
        conn.execute_unprepared(pragma).await.map_err(sql_err)?;
    }
    Ok(Arc::new(Sqlite { conn }))
}

async fn connect(url: &str) -> Result<Arc<dyn Store>, StoreError> {
    let conn = Database::connect(url).await.map_err(sql_err)?;
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
            Value::Decimal(value) => SeaValue::String(Some(Box::new(value.to_string()))),
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

fn leaf(filter: &Filter, params: &mut Vec<Value>) -> String {
    match filter.op {
        Op::Eq => {
            params.push(filter.value.clone());
            format!("\"{}\" = ?", filter.field)
        }
        Op::Ne => {
            params.push(filter.value.clone());
            format!("\"{}\" != ?", filter.field)
        }
        Op::More => {
            params.push(filter.value.clone());
            format!("\"{}\" > ?", filter.field)
        }
        Op::Less => {
            params.push(filter.value.clone());
            format!("\"{}\" < ?", filter.field)
        }
        Op::Like => {
            params.push(filter.value.clone());
            format!("LOWER(\"{}\") LIKE ?", filter.field)
        }
        Op::Bare => format!("\"{}\" IS NULL", filter.field),
        Op::At => match &filter.value {
            Value::DateTime(at) => {
                let start = at.date_naive().and_time(NaiveTime::MIN).and_utc();
                let end = start + TimeDelta::days(1);
                params.push(Value::datetime(start));
                params.push(Value::datetime(end));
                format!("\"{}\" >= ? AND \"{}\" < ?", filter.field, filter.field)
            }
            _ => "1 = 0".into(),
        },
        Op::In | Op::Out => todo!("phase 2"),
    }
}

fn tree(node: &Tree, params: &mut Vec<Value>) -> String {
    match node {
        Tree::Leaf(filter) => leaf(filter, params),
        Tree::And(parts) => {
            let mut out = Vec::new();
            for node in parts {
                let cond = tree(node, params);
                if !cond.is_empty() {
                    out.push(cond);
                }
            }
            if out.len() == 1 {
                out.pop().unwrap_or_default()
            } else if out.is_empty() {
                String::new()
            } else {
                format!("({})", out.join(" AND "))
            }
        }
        Tree::Or(parts) => {
            let mut out = Vec::new();
            for node in parts {
                let cond = tree(node, params);
                if !cond.is_empty() {
                    out.push(cond);
                }
            }
            if out.len() == 1 {
                out.pop().unwrap_or_default()
            } else if out.is_empty() {
                "1 = 0".into()
            } else {
                format!("({})", out.join(" OR "))
            }
        }
        Tree::Cut(inner) => {
            let cond = tree(inner, params);
            if cond.is_empty() {
                "1 = 1".into()
            } else {
                format!("NOT ({cond})")
            }
        }
    }
}

fn kind_of(schema: &Schema, name: Name) -> Result<ColumnKind, StoreError> {
    schema
        .fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| affinity(&field.kind))
        .ok_or_else(|| StoreError::Value(format!("unknown column {name}")))
}

fn sorts(sorts: &[Sort], schema: &Schema) -> String {
    if sorts.is_empty() {
        return format!("\"{}\"", schema.key());
    }
    sorts
        .iter()
        .map(|sort| match sort.order {
            Order::Asc => format!("\"{}\"", sort.field),
            Order::Desc => format!("\"{}\" DESC", sort.field),
        })
        .collect::<Vec<_>>()
        .join(", ")
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

    fn scan_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move {
            if query.mass.is_some() {
                todo!("phase 3")
            }
            let (head, kinds) = match &query.only {
                Only::All | Only::Lone => ("SELECT *".to_string(), schema.kinds()),
                Only::Some(names) if names.is_empty() => ("SELECT *".to_string(), schema.kinds()),
                Only::Some(names) => {
                    let mut kinds = Vec::with_capacity(names.len());
                    for name in names {
                        kinds.push(kind_of(&schema, *name)?);
                    }
                    let columns = names
                        .iter()
                        .map(|name| format!("\"{name}\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    (format!("SELECT {columns}"), kinds)
                }
            };
            let mut params = Vec::new();
            let mut sql = format!("{head} FROM \"{}\"", schema.table);
            let cond = tree(&query.tree, &mut params);
            if !cond.is_empty() {
                sql.push_str(&format!(" WHERE {cond}"));
            }
            sql.push_str(&format!(" ORDER BY {}", sorts(&query.sort, &schema)));
            if matches!(query.only, Only::Lone) {
                sql.push_str(" LIMIT 1");
            } else if query.page.count > 0 {
                sql.push_str(&format!(
                    " LIMIT {} OFFSET {}",
                    query.page.count, query.page.offset
                ));
            }
            let rows = self
                .conn
                .query_all(statement(&sql, &params))
                .await
                .map_err(sql_err)?;
            rows.iter().map(|row| read(row, &kinds)).collect()
        })
    }

    fn total_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move {
            if query.mass.is_some() {
                todo!("phase 3")
            }
            let mut params = Vec::new();
            let mut sql = format!("SELECT COUNT(*) FROM \"{}\"", schema.table);
            let cond = tree(&query.tree, &mut params);
            if !cond.is_empty() {
                sql.push_str(&format!(" WHERE {cond}"));
            }
            let rows = self
                .conn
                .query_all(statement(&sql, &params))
                .await
                .map_err(sql_err)?;
            match rows.first() {
                Some(row) => match row.try_get_by_index::<Option<i64>>(0) {
                    Ok(Some(total)) => Ok(total as usize),
                    _ => Err(StoreError::Value("no total".into())),
                },
                None => Err(StoreError::Value("no total".into())),
            }
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

    fn insert<'a>(
        &'a self,
        table: &'a str,
        columns: &'a [String],
        values: &'a [Value],
    ) -> BoxFuture<'a, Result<i64, StoreError>> {
        let table = table.to_string();
        let columns = columns.to_vec();
        let values = values.to_vec();
        Box::pin(async move {
            let cols = columns
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let marks = vec!["?"; columns.len()].join(", ");
            let sql = format!("INSERT INTO \"{table}\" ({cols}) VALUES ({marks}) RETURNING \"id\"");
            let rows = self
                .conn
                .query_all(statement(&sql, &values))
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
    use crate::{Field, Page, Table, Type};
    use chrono::NaiveDate;

    async fn open_db(name: &str) -> Arc<dyn Store> {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&path);
        open(&path).await.unwrap()
    }

    async fn store() -> Arc<dyn Store> {
        open_db("base").await
    }

    fn schema() -> Schema {
        Schema {
            table: Table("w"),
            fields: vec![
                Field::id(),
                Field::new("name", Type::Str),
                Field::new("age", Type::Int.optional()),
                Field::new("at", Type::Moment.optional()),
            ],
            rules: Vec::new(),
        }
    }

    fn ask(tree: Tree) -> Query {
        Query {
            tree,
            sort: Vec::new(),
            page: Page::all(),
            only: Only::All,
            mass: None,
        }
    }

    fn leaf(field: &'static str, op: Op, value: Value) -> Tree {
        Tree::Leaf(Filter {
            field: Name(field),
            op,
            value,
        })
    }

    async fn seed() -> Arc<dyn Store> {
        let db = open_db("trees").await;
        db.execute(&schema().ddl(), &[]).await.unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_time(NaiveTime::MIN)
            .and_utc();
        let rows = vec![
            (Value::str("ann"), Value::int(30), Value::datetime(day)),
            (Value::str("bob"), Value::int(40), Value::Null),
            (Value::str("ann"), Value::int(30), Value::datetime(day)),
            (Value::str("zed"), Value::Null, Value::Null),
        ];
        for (name, age, at) in rows {
            db.execute(
                "INSERT INTO w (name, age, at) VALUES (?, ?, ?)",
                &[name, age, at],
            )
            .await
            .unwrap();
        }
        db
    }

    #[tokio::test]
    async fn trees() {
        let db = seed().await;
        let schema = schema();
        let total = |tree: Tree| {
            let db = db.clone();
            let schema = schema.clone();
            async move { db.total_query(&schema, &ask(tree)).await.unwrap() }
        };
        assert_eq!(total(Tree::And(Vec::new())).await, 4);
        assert_eq!(total(leaf("name", Op::Eq, Value::str("ann"))).await, 2);
        assert_eq!(total(leaf("name", Op::Like, Value::str("%ann%"))).await, 2);
        assert_eq!(
            total(Tree::Or(vec![
                leaf("name", Op::Eq, Value::str("ann")),
                leaf("age", Op::Eq, Value::int(40)),
            ]))
            .await,
            3
        );
        assert_eq!(
            total(Tree::Cut(Box::new(leaf("name", Op::Eq, Value::str("ann"))))).await,
            2
        );
        assert_eq!(total(leaf("name", Op::Ne, Value::str("bob"))).await, 3);
        assert_eq!(total(leaf("age", Op::More, Value::int(30))).await, 1);
        assert_eq!(total(leaf("age", Op::Less, Value::int(40))).await, 2);
        assert_eq!(total(leaf("age", Op::Bare, Value::Null)).await, 1);
        let day = NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_time(NaiveTime::MIN)
            .and_utc();
        assert_eq!(total(leaf("at", Op::At, Value::datetime(day))).await, 2);
        let rows = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: vec![Sort {
                        field: Name("age"),
                        order: Order::Desc,
                    }],
                    page: Page {
                        count: 2,
                        offset: 0,
                    },
                    only: Only::All,
                    mass: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].str(1).unwrap(), "bob");
        let rows = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: Vec::new(),
                    page: Page::all(),
                    only: Only::Some(vec![Name("name")]),
                    mass: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.values.len() == 1));
        assert_eq!(rows[0].str(0).unwrap(), "ann");
        let rows = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: vec![Sort {
                        field: Name("name"),
                        order: Order::Asc,
                    }],
                    page: Page::all(),
                    only: Only::Lone,
                    mass: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].str(1).unwrap(), "ann");
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
