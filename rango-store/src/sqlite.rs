use std::{path::Path, sync::Arc};

use chrono::{NaiveTime, TimeDelta};
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr,
    QueryResult, Statement, TransactionTrait, Value as SeaValue,
};
use tokio::sync::Mutex;

use crate::{
    BoxFuture, Column, ColumnKind, Field, Filter, Key, Mass, Name, Only, Op, Order, Query, Row,
    Rows, Schema, Sort, Store, StoreError, Tree, Type, Value, many,
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
    let field = schema
        .fields
        .iter()
        .find(|field| field.name == name)
        .ok_or_else(|| StoreError::Value(format!("unknown column {name}")))?;
    if many(&field.kind) {
        return Err(StoreError::Value(format!("virtual column {name}")));
    }
    Ok(affinity(&field.kind))
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

fn affinity(kind: &Type) -> ColumnKind {
    match kind {
        Type::Id | Type::Int | Type::Moment | Type::Bool => ColumnKind::Integer,
        Type::Float => ColumnKind::Real,
        Type::Str | Type::Key | Type::Decimal => ColumnKind::Text,
        Type::Many => unreachable!("virtual field has no column"),
        Type::Opt(inner) => affinity(inner),
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
        Type::Many => unreachable!("virtual field has no column"),
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

fn kinds(schema: &Schema) -> Vec<ColumnKind> {
    schema
        .fields
        .iter()
        .filter(|field| !many(&field.kind))
        .map(|field| affinity(&field.kind))
        .collect()
}

fn ddl(schema: &Schema) -> String {
    let columns: Vec<String> = schema
        .fields
        .iter()
        .filter(|field| !many(&field.kind))
        .map(column)
        .collect();
    format!(
        "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
        schema.table,
        columns.join(", ")
    )
}

fn alter(schema: &Schema, have: &[Column]) -> Vec<String> {
    let moved = moved(schema, have);
    let mut out = Vec::new();
    for field in &schema.fields {
        if field.kind == Type::Id || many(&field.kind) {
            continue;
        }
        if !have.iter().any(|col| col.name == field.name.as_str())
            && !moved.iter().any(|(_, name)| name == field.name.as_str())
        {
            out.push(format!(
                "ALTER TABLE \"{}\" ADD COLUMN {}",
                schema.table,
                column(field)
            ));
        }
    }
    out
}

fn drop(schema: &Schema, have: &[Column]) -> Vec<String> {
    let moved = moved(schema, have);
    let mut out = Vec::new();
    for col in have {
        if col.name == "id" {
            continue;
        }
        if !schema
            .fields
            .iter()
            .any(|field| field.name.as_str() == col.name.as_str())
            && !moved.iter().any(|(name, _)| name == &col.name)
        {
            out.push(format!(
                "ALTER TABLE \"{}\" DROP COLUMN \"{}\"",
                schema.table, col.name
            ));
        }
    }
    out
}

fn rename(schema: &Schema, have: &[Column]) -> Vec<String> {
    moved(schema, have)
        .into_iter()
        .map(|(old, name)| {
            format!(
                "ALTER TABLE \"{}\" RENAME COLUMN \"{old}\" TO \"{name}\"",
                schema.table
            )
        })
        .collect()
}

fn moved(schema: &Schema, have: &[Column]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for kind in [ColumnKind::Integer, ColumnKind::Real, ColumnKind::Text] {
        let gone: Vec<&String> = have
            .iter()
            .filter(|col| {
                col.name != "id"
                    && col.kind == kind
                    && !schema
                        .fields
                        .iter()
                        .any(|field| field.name.as_str() == col.name.as_str())
            })
            .map(|col| &col.name)
            .collect();
        let fresh: Vec<&Field> = schema
            .fields
            .iter()
            .filter(|field| {
                field.kind != Type::Id
                    && !many(&field.kind)
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

async fn run_execute(
    conn: &impl ConnectionTrait,
    sql: &str,
    params: &[Value],
) -> Result<usize, StoreError> {
    conn.execute(statement(sql, params))
        .await
        .map(|done| done.rows_affected() as usize)
        .map_err(sql_err)
}

async fn run_fetch(
    conn: &impl ConnectionTrait,
    sql: &str,
    params: &[Value],
    kinds: &[ColumnKind],
) -> Result<Rows, StoreError> {
    let rows = conn
        .query_all(statement(sql, params))
        .await
        .map_err(sql_err)?;
    rows.iter().map(|row| read(row, kinds)).collect()
}

async fn run_scan(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    query: &Query,
) -> Result<Rows, StoreError> {
    if query.mass.is_some() {
        return Err(StoreError::Unsupported("mass needs mass()".into()));
    }
    let (head, kinds) = match &query.only {
        Only::All | Only::Lone => ("SELECT *".to_string(), kinds(schema)),
        Only::Some(names) if names.is_empty() => ("SELECT *".to_string(), kinds(schema)),
        Only::Some(names) => {
            let mut kinds = Vec::with_capacity(names.len());
            for name in names {
                kinds.push(kind_of(schema, *name)?);
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
    sql.push_str(&format!(" ORDER BY {}", sorts(&query.sort, schema)));
    if matches!(query.only, Only::Lone) {
        sql.push_str(" LIMIT 1");
    } else if query.page.count > 0 {
        sql.push_str(&format!(
            " LIMIT {} OFFSET {}",
            query.page.count, query.page.offset
        ));
    }
    let rows = conn
        .query_all(statement(&sql, &params))
        .await
        .map_err(sql_err)?;
    rows.iter().map(|row| read(row, &kinds)).collect()
}

async fn run_total(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    query: &Query,
) -> Result<usize, StoreError> {
    if query.mass.is_some() {
        return Err(StoreError::Unsupported("mass needs mass()".into()));
    }
    let mut params = Vec::new();
    let mut sql = format!("SELECT COUNT(*) FROM \"{}\"", schema.table);
    let cond = tree(&query.tree, &mut params);
    if !cond.is_empty() {
        sql.push_str(&format!(" WHERE {cond}"));
    }
    let rows = conn
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
}

async fn run_define(conn: &impl ConnectionTrait, schema: &Schema) -> Result<(), StoreError> {
    run_execute(conn, &ddl(schema), &[]).await.map(|_| ())
}

async fn run_create(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    batch: &[Vec<(Name, Value)>],
) -> Result<Vec<Key>, StoreError> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }
    let keyed = schema
        .fields
        .iter()
        .any(|field| matches!(field.kind.flat(), Type::Key));
    if keyed {
        let head = &batch[0];
        let columns = head
            .iter()
            .map(|pair| format!("\"{}\"", pair.0))
            .collect::<Vec<_>>()
            .join(", ");
        let mut params = Vec::new();
        let mut groups = Vec::new();
        for cells in batch {
            groups.push(format!("({})", vec!["?"; cells.len()].join(", ")));
            params.extend(cells.iter().map(|pair| pair.1.clone()));
        }
        let sql = format!(
            "INSERT OR REPLACE INTO \"{}\" ({columns}) VALUES {}",
            schema.table,
            groups.join(", ")
        );
        run_execute(conn, &sql, &params).await?;
        let id = schema.key();
        let mut out = Vec::with_capacity(batch.len());
        for cells in batch {
            let value = cells.iter().find(|pair| pair.0 == id).map(|pair| &pair.1);
            match value {
                Some(value) => out.push(Key::of(value)?),
                None => return Err(StoreError::Value("missing key".into())),
            }
        }
        return Ok(out);
    }
    let mut out = Vec::with_capacity(batch.len());
    for cells in batch {
        let columns = cells
            .iter()
            .map(|pair| pair.0.to_string())
            .collect::<Vec<_>>();
        let params = cells.iter().map(|pair| pair.1.clone()).collect::<Vec<_>>();
        let id = run_insert(conn, schema.table.as_str(), &columns, &params).await?;
        out.push(Key::Int(id));
    }
    Ok(out)
}

async fn run_replace(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    key: &Value,
    cells: &[(Name, Value)],
) -> Result<(), StoreError> {
    let mut sets = Vec::with_capacity(cells.len());
    let mut params = Vec::with_capacity(cells.len() + 1);
    for (name, value) in cells {
        sets.push(format!("\"{name}\" = ?"));
        params.push(value.clone());
    }
    params.push(key.clone());
    let sql = format!(
        "UPDATE \"{}\" SET {} WHERE \"{}\" = ?",
        schema.table,
        sets.join(", "),
        schema.key()
    );
    run_execute(conn, &sql, &params).await?;
    Ok(())
}

async fn run_upsert(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    batch: &[Vec<(Name, Value)>],
) -> Result<usize, StoreError> {
    if batch.is_empty() {
        return Ok(0);
    }
    if !schema
        .fields
        .iter()
        .any(|field| matches!(field.kind.flat(), Type::Key))
    {
        let created = run_create(conn, schema, batch).await?;
        return Ok(created.len());
    }
    let head = &batch[0];
    let columns = head
        .iter()
        .map(|pair| format!("\"{}\"", pair.0))
        .collect::<Vec<_>>()
        .join(", ");
    let mut params = Vec::new();
    let mut groups = Vec::new();
    for cells in batch {
        groups.push(format!("({})", vec!["?"; cells.len()].join(", ")));
        params.extend(cells.iter().map(|pair| pair.1.clone()));
    }
    let key = schema.key();
    let sets = head
        .iter()
        .filter(|pair| pair.0 != key)
        .map(|pair| format!("\"{}\" = excluded.\"{}\"", pair.0, pair.0))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO \"{}\" ({columns}) VALUES {} ON CONFLICT(\"{key}\") DO UPDATE SET {sets}",
        schema.table,
        groups.join(", ")
    );
    run_execute(conn, &sql, &params).await
}

async fn run_remove(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    key: &Value,
) -> Result<(), StoreError> {
    let sql = format!(
        "DELETE FROM \"{}\" WHERE \"{}\" = ?",
        schema.table,
        schema.key()
    );
    run_execute(conn, &sql, std::slice::from_ref(key)).await?;
    Ok(())
}

async fn run_evolve(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    trim: bool,
) -> Result<usize, StoreError> {
    let mut done = 0;
    run_define(conn, schema).await?;
    done += 1;
    let have = run_columns(conn, schema.table.as_str()).await?;
    for sql in rename(schema, &have) {
        run_execute(conn, &sql, &[]).await?;
        done += 1;
    }
    for sql in alter(schema, &have) {
        run_execute(conn, &sql, &[]).await?;
        done += 1;
    }
    if trim {
        for sql in drop(schema, &have) {
            run_execute(conn, &sql, &[]).await?;
            done += 1;
        }
    }
    Ok(done)
}

async fn run_mass(
    conn: &impl ConnectionTrait,
    schema: &Schema,
    query: &Query,
) -> Result<Value, StoreError> {
    let mass = match query.mass {
        Some(mass) => mass,
        None => return Err(StoreError::Unsupported("mass needs a mass".into())),
    };
    let (head, kinds) = match mass {
        Mass::Count => ("COUNT(*)".to_string(), vec![ColumnKind::Integer]),
        Mass::Sum(name) => (format!("SUM(\"{name}\")"), vec![kind_of(schema, name)?]),
        Mass::Mean(name) => (format!("AVG(\"{name}\")"), vec![ColumnKind::Real]),
        Mass::Low(name) => (format!("MIN(\"{name}\")"), vec![kind_of(schema, name)?]),
        Mass::High(name) => (format!("MAX(\"{name}\")"), vec![kind_of(schema, name)?]),
    };
    let mut params = Vec::new();
    let mut sql = format!("SELECT {head} FROM \"{}\"", schema.table);
    let cond = tree(&query.tree, &mut params);
    if !cond.is_empty() {
        sql.push_str(&format!(" WHERE {cond}"));
    }
    let rows = conn
        .query_all(statement(&sql, &params))
        .await
        .map_err(sql_err)?;
    let rows: Vec<Row> = rows
        .iter()
        .map(|row| read(row, &kinds))
        .collect::<Result<_, _>>()?;
    Ok(rows
        .first()
        .and_then(|row| row.get(0).cloned())
        .unwrap_or(Value::Null))
}

async fn run_columns(conn: &impl ConnectionTrait, table: &str) -> Result<Vec<Column>, StoreError> {
    let rows = conn
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
}

async fn run_last_id(conn: &impl ConnectionTrait) -> Result<i64, StoreError> {
    let rows = conn
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
}

async fn run_insert(
    conn: &impl ConnectionTrait,
    table: &str,
    columns: &[String],
    values: &[Value],
) -> Result<i64, StoreError> {
    let cols = columns
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let marks = vec!["?"; columns.len()].join(", ");
    let sql = format!("INSERT INTO \"{table}\" ({cols}) VALUES ({marks}) RETURNING \"id\"");
    let rows = conn
        .query_all(statement(&sql, values))
        .await
        .map_err(sql_err)?;
    match rows
        .first()
        .and_then(|row| row.try_get_by_index::<Option<i64>>(0).ok().flatten())
    {
        Some(id) => Ok(id),
        None => Err(StoreError::Value("no last id".into())),
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
        Box::pin(async move { run_execute(&self.conn, &sql, &params).await })
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
        Box::pin(async move { run_fetch(&self.conn, &sql, &params, &kinds).await })
    }

    fn scan_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { run_scan(&self.conn, &schema, &query).await })
    }

    fn total_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { run_total(&self.conn, &schema, &query).await })
    }

    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        Box::pin(async move { run_define(&self.conn, &schema).await })
    }

    fn create<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<Vec<Key>, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move { run_create(&self.conn, &schema, &batch).await })
    }

    fn upsert<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move { run_upsert(&self.conn, &schema, &batch).await })
    }

    fn replace<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
        cells: &'a [(Name, Value)],
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        let key = key.value();
        let cells = cells.to_vec();
        Box::pin(async move { run_replace(&self.conn, &schema, &key, &cells).await })
    }

    fn remove<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        let key = key.value();
        Box::pin(async move { run_remove(&self.conn, &schema, &key).await })
    }

    fn evolve<'a>(
        &'a self,
        schema: &'a Schema,
        trim: bool,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        Box::pin(async move { run_evolve(&self.conn, &schema, trim).await })
    }

    fn mass<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Value, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { run_mass(&self.conn, &schema, &query).await })
    }

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
        let table = table.to_string();
        Box::pin(async move { run_columns(&self.conn, &table).await })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move { run_last_id(&self.conn).await })
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
        Box::pin(async move { run_insert(&self.conn, &table, &columns, &values).await })
    }

    fn deal<'a>(&'a self) -> BoxFuture<'a, Result<Arc<dyn Store>, StoreError>> {
        Box::pin(async move {
            let txn = self.conn.begin().await.map_err(sql_err)?;
            Ok(Arc::new(Trade {
                txn: Mutex::new(Some(txn)),
            }) as Arc<dyn Store>)
        })
    }
}

struct Trade {
    txn: Mutex<Option<DatabaseTransaction>>,
}

impl Store for Trade {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_execute(txn, &sql, &params).await
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
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_fetch(txn, &sql, &params, &kinds).await
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
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_scan(txn, &schema, &query).await
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
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_total(txn, &schema, &query).await
        })
    }

    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_define(txn, &schema).await
        })
    }

    fn create<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<Vec<Key>, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_create(txn, &schema, &batch).await
        })
    }

    fn upsert<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_upsert(txn, &schema, &batch).await
        })
    }

    fn replace<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
        cells: &'a [(Name, Value)],
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        let key = key.value();
        let cells = cells.to_vec();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_replace(txn, &schema, &key, &cells).await
        })
    }

    fn remove<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        let key = key.value();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_remove(txn, &schema, &key).await
        })
    }

    fn evolve<'a>(
        &'a self,
        schema: &'a Schema,
        trim: bool,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_evolve(txn, &schema, trim).await
        })
    }

    fn mass<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Value, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_mass(txn, &schema, &query).await
        })
    }

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
        let table = table.to_string();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_columns(txn, &table).await
        })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_last_id(txn).await
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
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_insert(txn, &table, &columns, &values).await
        })
    }

    fn deal<'a>(&'a self) -> BoxFuture<'a, Result<Arc<dyn Store>, StoreError>> {
        Box::pin(async { Err(StoreError::Unsupported("nested deal".into())) })
    }

    fn settle(self: Arc<Self>, commit: bool) -> BoxFuture<'static, Result<(), StoreError>> {
        Box::pin(async move {
            let mut guard = self.txn.lock().await;
            let txn = match guard.take() {
                Some(txn) => txn,
                None => return Ok(()),
            };
            if commit {
                txn.commit().await.map_err(sql_err)
            } else {
                txn.rollback().await.map_err(sql_err)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, Page, Table};
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

    fn col(name: &str, kind: ColumnKind) -> Column {
        Column {
            name: name.to_string(),
            kind,
        }
    }

    fn posts() -> Schema {
        Schema {
            table: Table("posts"),
            fields: vec![Field::id(), Field::new("title", Type::Str)],
            rules: Vec::new(),
        }
    }

    fn keyed() -> Schema {
        Schema {
            table: Table("products"),
            fields: vec![Field::key("sku"), Field::new("price", Type::Decimal)],
            rules: Vec::new(),
        }
    }

    #[test]
    fn renders() {
        assert_eq!(
            ddl(&posts()),
            "CREATE TABLE IF NOT EXISTS \"posts\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"title\" TEXT NOT NULL)"
        );
        assert_eq!(
            ddl(&keyed()),
            "CREATE TABLE IF NOT EXISTS \"products\" (\"sku\" TEXT PRIMARY KEY, \"price\" TEXT NOT NULL)"
        );
        assert_eq!(kinds(&posts()), vec![ColumnKind::Integer, ColumnKind::Text]);
    }

    #[test]
    fn alters() {
        let schema = posts();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
        ];
        assert!(alter(&schema, &have).is_empty());
        let missing = vec![col("id", ColumnKind::Integer)];
        assert_eq!(alter(&schema, &missing).len(), 1);
    }

    #[test]
    fn drops() {
        let schema = posts();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
            col("junk", ColumnKind::Text),
        ];
        assert_eq!(drop(&schema, &have).len(), 1);
        assert!(drop(&schema, &have[..2]).is_empty());
    }

    #[test]
    fn renames() {
        let schema = posts();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
        ];
        assert_eq!(rename(&schema, &have).len(), 1);
        assert!(alter(&schema, &have).is_empty());
        assert!(drop(&schema, &have).is_empty());
        let mixed = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
            col("age", ColumnKind::Integer),
        ];
        assert_eq!(rename(&schema, &mixed).len(), 1);
        assert!(alter(&schema, &mixed).is_empty());
        assert_eq!(drop(&schema, &mixed).len(), 1);
    }

    #[tokio::test]
    async fn upserts() {
        let db = open_db("upserts").await;
        let schema = Schema {
            table: Table("p"),
            fields: vec![Field::key("code"), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let rows = vec![
            vec![(Name("code"), Value::str("x")), (Name("name"), Value::str("n1"))],
            vec![(Name("code"), Value::str("x")), (Name("name"), Value::str("n2"))],
            vec![(Name("code"), Value::str("y")), (Name("name"), Value::str("m"))],
        ];
        db.create(&schema, &rows).await.unwrap();
        let query = |api| async {
            db.total_query(&schema, &ask(api)).await.unwrap()
        };
        assert_eq!(query(Tree::And(Vec::new())).await, 2);
        let row = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: Vec::new(),
                    page: Page::all(),
                    only: Only::All,
                    mass: None,
                },
            )
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.str(0).unwrap() == "x")
            .expect("x row");
        assert_eq!(row.str(1).unwrap(), "n2");
    }

    #[tokio::test]
    async fn upsert_merge() {
        let db = open_db("upsert_merge").await;
        let schema = Schema {
            table: Table("p2"),
            fields: vec![Field::key("code"), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let one = vec![vec![
            (Name("code"), Value::str("x")),
            (Name("name"), Value::str("n1")),
        ]];
        assert_eq!(db.upsert(&schema, &one).await.unwrap(), 1);
        let two = vec![
            vec![
                (Name("code"), Value::str("x")),
                (Name("name"), Value::str("n2")),
            ],
            vec![
                (Name("code"), Value::str("y")),
                (Name("name"), Value::str("m")),
            ],
        ];
        assert_eq!(db.upsert(&schema, &two).await.unwrap(), 2);
        let query = |api| async { db.total_query(&schema, &ask(api)).await.unwrap() };
        assert_eq!(query(Tree::And(Vec::new())).await, 2);
        let row = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: Vec::new(),
                    page: Page::all(),
                    only: Only::All,
                    mass: None,
                },
            )
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.str(0).unwrap() == "x")
            .expect("x row");
        assert_eq!(row.str(1).unwrap(), "n2");
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
        db.define(&schema()).await.unwrap();
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
    async fn writes() {
        let db = open_db("writes").await;
        let schema = Schema {
            table: Table("p"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let keys = db
            .create(
                &schema,
                &[
                    vec![(Name("name"), Value::str("a"))],
                    vec![(Name("name"), Value::str("b"))],
                ],
            )
            .await
            .unwrap();
        assert_eq!(keys, vec![Key::Int(1), Key::Int(2)]);
        let keyed = Schema {
            table: Table("k"),
            fields: vec![Field::key("sku"), Field::new("price", Type::Int)],
            rules: Vec::new(),
        };
        db.define(&keyed).await.unwrap();
        let keys = db
            .create(
                &keyed,
                &[vec![
                    (Name("sku"), Value::str("s1")),
                    (Name("price"), Value::int(5)),
                ]],
            )
            .await
            .unwrap();
        assert_eq!(keys, vec![Key::Text("s1".into())]);
        db.replace(&schema, &Key::Int(1), &[(Name("name"), Value::str("a2"))])
            .await
            .unwrap();
        let rows = db
            .scan_query(&schema, &ask(Tree::And(Vec::new())))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].str(1).unwrap(), "a2");
        db.remove(&schema, &Key::Int(2)).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn evolves() {
        let db = open_db("evolves").await;
        let one = Schema {
            table: Table("e"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        assert_eq!(db.evolve(&one, false).await.unwrap(), 1);
        assert_eq!(db.evolve(&one, false).await.unwrap(), 1);
        let two = Schema {
            table: Table("e"),
            fields: vec![
                Field::id(),
                Field::new("name", Type::Str),
                Field::new("age", Type::Int.optional()),
            ],
            rules: Vec::new(),
        };
        assert_eq!(db.evolve(&two, false).await.unwrap(), 2);
        let names = db
            .columns("e")
            .await
            .unwrap()
            .into_iter()
            .map(|col| col.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"age".to_string()));
        assert_eq!(db.evolve(&one, false).await.unwrap(), 1);
        assert_eq!(db.evolve(&one, true).await.unwrap(), 2);
        let names = db
            .columns("e")
            .await
            .unwrap()
            .into_iter()
            .map(|col| col.name)
            .collect::<Vec<_>>();
        assert!(!names.contains(&"age".to_string()));
    }

    #[tokio::test]
    async fn masses() {
        let db = open_db("masses").await;
        let schema = Schema {
            table: Table("g"),
            fields: vec![
                Field::id(),
                Field::new("name", Type::Str),
                Field::new("age", Type::Int.optional()),
            ],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        for (name, age) in [
            (Value::str("ann"), Value::int(30)),
            (Value::str("bob"), Value::int(40)),
            (Value::str("cid"), Value::Null),
        ] {
            db.execute("INSERT INTO g (name, age) VALUES (?, ?)", &[name, age])
                .await
                .unwrap();
        }
        let mass = |mass: Mass, tree: Tree| {
            let db = db.clone();
            let schema = schema.clone();
            async move {
                db.mass(
                    &schema,
                    &Query {
                        tree,
                        sort: Vec::new(),
                        page: Page::all(),
                        only: Only::All,
                        mass: Some(mass),
                    },
                )
                .await
                .unwrap()
            }
        };
        assert_eq!(
            mass(Mass::Count, Tree::And(Vec::new())).await,
            Value::int(3)
        );
        assert_eq!(
            mass(Mass::Count, leaf("name", Op::Eq, Value::str("ann"))).await,
            Value::int(1)
        );
        assert_eq!(
            mass(Mass::Sum(Name("age")), Tree::And(Vec::new())).await,
            Value::int(70)
        );
        assert_eq!(
            mass(Mass::Low(Name("age")), Tree::And(Vec::new())).await,
            Value::int(30)
        );
        assert_eq!(
            mass(Mass::High(Name("age")), Tree::And(Vec::new())).await,
            Value::int(40)
        );
        match mass(Mass::Mean(Name("age")), Tree::And(Vec::new())).await {
            Value::Float(mean) => assert!((mean - 35.0).abs() < 0.001),
            _ => panic!("not a mean"),
        }
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

    #[tokio::test]
    async fn wipes() {
        use crate::Action;
        let db = open_db("wipes").await;
        let schema = Schema {
            table: Table("w"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let keys = db
            .create(
                &schema,
                &[
                    vec![(Name("name"), Value::str("a"))],
                    vec![(Name("name"), Value::str("b"))],
                    vec![(Name("name"), Value::str("c"))],
                ],
            )
            .await
            .unwrap();
        let deed = Action::wipe();
        assert_eq!(deed.name, Name("wipe"));
        assert!(deed.logged);
        let msg = (deed.run)(db.clone(), schema.clone(), keys[..2].to_vec())
            .await
            .unwrap();
        assert_eq!(msg, "Deleted 2 rows.");
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
        let msg = (deed.run)(db.clone(), schema, keys[2..].to_vec())
            .await
            .unwrap();
        assert_eq!(msg, "Deleted 1 row.");
    }

    fn deals() -> Schema {
        Schema {
            table: Table("deals"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        }
    }

    fn one() -> Vec<Vec<(Name, Value)>> {
        vec![vec![(Name("name"), Value::str("a"))]]
    }

    #[tokio::test]
    async fn commits() {
        let db = open_db("commits").await;
        let schema = deals();
        db.define(&schema).await.unwrap();
        let tx = db.deal().await.unwrap();
        tx.create(&schema, &one()).await.unwrap();
        assert_eq!(
            tx.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
        tx.settle(true).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn isolates() {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-isolates.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first = open_wal(&path).await.unwrap();
        let schema = deals();
        first.define(&schema).await.unwrap();
        let tx = first.deal().await.unwrap();
        tx.create(&schema, &one()).await.unwrap();
        let second = open_wal(&path).await.unwrap();
        let ask_all = || ask(Tree::And(Vec::new()));
        assert_eq!(second.total_query(&schema, &ask_all()).await.unwrap(), 0);
        tx.settle(true).await.unwrap();
        assert_eq!(second.total_query(&schema, &ask_all()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn aborts() {
        let db = open_db("aborts").await;
        let schema = deals();
        db.define(&schema).await.unwrap();
        let tx = db.deal().await.unwrap();
        tx.create(&schema, &one()).await.unwrap();
        tx.settle(false).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn nested() {
        let db = open_db("nested").await;
        let tx = db.deal().await.unwrap();
        let err = match tx.deal().await {
            Err(err) => err,
            Ok(_) => panic!("nested deal"),
        };
        assert!(matches!(err, StoreError::Unsupported(_)));
        tx.settle(false).await.unwrap();
    }

    #[tokio::test]
    async fn abandons() {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-abandons.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = open_wal(&path).await.unwrap();
        let schema = deals();
        db.define(&schema).await.unwrap();
        {
            let tx = db.deal().await.unwrap();
            tx.create(&schema, &one()).await.unwrap();
        }
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            0
        );
    }
}
