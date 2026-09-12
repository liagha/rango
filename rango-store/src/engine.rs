use std::sync::Arc;

use chrono::{NaiveTime, TimeDelta};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, QueryResult, Statement,
    TransactionTrait, Value as SeaValue,
};
use tokio::sync::Mutex;

use crate::{
    BoxFuture, Column, ColumnKind, Field, Filter, Key, Mass, Name, Only, Op, Order, Query, Row,
    Rows, Schema, Sort, Store, StoreError, Tree, Type, Value, many,
};

pub(crate) trait Dialect: Clone + Send + Sync + 'static {
    fn backend(&self) -> DbBackend;
    fn mark(&self, at: usize) -> String;
    fn sql(&self, kind: &Type) -> &'static str;
    fn create(
        &self,
        schema: &Schema,
        head: &[(Name, Value)],
        columns: &str,
        groups: &str,
    ) -> String;
    fn introspect(&self, table: &str) -> String;
    fn name_at(&self) -> usize;
    fn type_at(&self) -> usize;
    fn reflect(&self, sql: &str) -> ColumnKind;
    fn latest(&self) -> Option<&'static str>;
    fn sum(&self, kind: ColumnKind, column: &str) -> String;
    fn mean(&self, kind: ColumnKind, column: &str) -> String;
}

pub(crate) fn sql_err(err: DbErr) -> StoreError {
    StoreError::Sql(err.to_string())
}

pub(crate) fn bind(values: &[Value]) -> Vec<SeaValue> {
    values
        .iter()
        .map(|value| match value {
            Value::Null => SeaValue::String(None),
            Value::Int(value) => SeaValue::BigInt(Some(*value)),
            Value::Float(value) => SeaValue::Double(Some(*value)),
            Value::Str(value) => SeaValue::String(Some(Box::new(value.clone()))),
            Value::Bool(value) => SeaValue::BigInt(Some(*value as i64)),
            Value::DateTime(at) => SeaValue::BigInt(Some(at.timestamp())),
            Value::Decimal(value) => SeaValue::String(Some(Box::new(value.to_string()))),
        })
        .collect()
}

pub(crate) fn read(row: &QueryResult, kinds: &[ColumnKind]) -> Result<Row, StoreError> {
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

pub(crate) fn leaf<D: Dialect>(
    dialect: &D,
    filter: &Filter,
    params: &mut Vec<Value>,
) -> String {
    match filter.op {
        Op::Eq => {
            params.push(filter.value.clone());
            format!("\"{}\" = {}", filter.field, dialect.mark(params.len() - 1))
        }
        Op::Ne => {
            params.push(filter.value.clone());
            format!("\"{}\" != {}", filter.field, dialect.mark(params.len() - 1))
        }
        Op::More => {
            params.push(filter.value.clone());
            format!("\"{}\" > {}", filter.field, dialect.mark(params.len() - 1))
        }
        Op::Less => {
            params.push(filter.value.clone());
            format!("\"{}\" < {}", filter.field, dialect.mark(params.len() - 1))
        }
        Op::Like => {
            params.push(filter.value.clone());
            format!(
                "LOWER(\"{}\") LIKE {}",
                filter.field,
                dialect.mark(params.len() - 1)
            )
        }
        Op::Bare => format!("\"{}\" IS NULL", filter.field),
        Op::At => match &filter.value {
            Value::DateTime(at) => {
                let start = at.date_naive().and_time(NaiveTime::MIN).and_utc();
                let end = start + TimeDelta::days(1);
                params.push(Value::datetime(start));
                let lo = dialect.mark(params.len() - 1);
                params.push(Value::datetime(end));
                let hi = dialect.mark(params.len() - 1);
                format!("\"{}\" >= {lo} AND \"{}\" < {hi}", filter.field, filter.field)
            }
            _ => "1 = 0".into(),
        },
    }
}

pub(crate) fn tree<D: Dialect>(dialect: &D, node: &Tree, params: &mut Vec<Value>) -> String {
    match node {
        Tree::Leaf(filter) => leaf(dialect, filter, params),
        Tree::And(parts) => {
            let mut out = Vec::new();
            for node in parts {
                let cond = tree(dialect, node, params);
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
                let cond = tree(dialect, node, params);
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
            let cond = tree(dialect, inner, params);
            if cond.is_empty() {
                "1 = 1".into()
            } else {
                format!("NOT ({cond})")
            }
        }
    }
}

pub(crate) fn kind_of(schema: &Schema, name: Name) -> Result<ColumnKind, StoreError> {
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

pub(crate) fn sorts(sorts: &[Sort], schema: &Schema) -> String {
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

pub(crate) fn statement<D: Dialect>(dialect: &D, sql: &str, params: &[Value]) -> Statement {
    Statement::from_sql_and_values(dialect.backend(), sql, bind(params))
}

pub(crate) fn affinity(kind: &Type) -> ColumnKind {
    match kind {
        Type::Id | Type::Int | Type::Moment | Type::Bool => ColumnKind::Integer,
        Type::Float => ColumnKind::Real,
        Type::Str | Type::Key | Type::Decimal => ColumnKind::Text,
        Type::Many => unreachable!("virtual field has no column"),
        Type::Opt(inner) => affinity(inner),
    }
}

pub(crate) fn literal(value: &Value) -> String {
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

pub(crate) fn column<D: Dialect>(dialect: &D, field: &Field) -> String {
    let mut base = dialect.sql(&field.kind).to_string();
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

pub(crate) fn kinds(schema: &Schema) -> Vec<ColumnKind> {
    schema
        .fields
        .iter()
        .filter(|field| !many(&field.kind))
        .map(|field| affinity(&field.kind))
        .collect()
}

pub(crate) fn ddl<D: Dialect>(dialect: &D, schema: &Schema) -> String {
    let columns: Vec<String> = schema
        .fields
        .iter()
        .filter(|field| !many(&field.kind))
        .map(|field| column(dialect, field))
        .collect();
    format!(
        "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
        schema.table,
        columns.join(", ")
    )
}

pub(crate) fn alter<D: Dialect>(
    dialect: &D,
    schema: &Schema,
    have: &[Column],
) -> Vec<String> {
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
                column(dialect, field)
            ));
        }
    }
    out
}

pub(crate) fn drop(schema: &Schema, have: &[Column]) -> Vec<String> {
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

pub(crate) fn rename(schema: &Schema, have: &[Column]) -> Vec<String> {
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

pub(crate) fn moved(schema: &Schema, have: &[Column]) -> Vec<(String, String)> {
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

async fn run_execute<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    sql: &str,
    params: &[Value],
) -> Result<usize, StoreError> {
    conn.execute(statement(dialect, sql, params))
        .await
        .map(|done| done.rows_affected() as usize)
        .map_err(sql_err)
}

async fn run_fetch<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    sql: &str,
    params: &[Value],
    kinds: &[ColumnKind],
) -> Result<Rows, StoreError> {
    let rows = conn
        .query_all(statement(dialect, sql, params))
        .await
        .map_err(sql_err)?;
    rows.iter().map(|row| read(row, kinds)).collect()
}

async fn run_scan<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
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
    let cond = tree(dialect, &query.tree, &mut params);
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
        .query_all(statement(dialect, &sql, &params))
        .await
        .map_err(sql_err)?;
    rows.iter().map(|row| read(row, &kinds)).collect()
}

async fn run_total<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    schema: &Schema,
    query: &Query,
) -> Result<usize, StoreError> {
    if query.mass.is_some() {
        return Err(StoreError::Unsupported("mass needs mass()".into()));
    }
    let mut params = Vec::new();
    let mut sql = format!("SELECT COUNT(*) FROM \"{}\"", schema.table);
    let cond = tree(dialect, &query.tree, &mut params);
    if !cond.is_empty() {
        sql.push_str(&format!(" WHERE {cond}"));
    }
    let rows = conn
        .query_all(statement(dialect, &sql, &params))
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

async fn run_define<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    schema: &Schema,
) -> Result<(), StoreError> {
    run_execute(conn, dialect, &ddl(dialect, schema), &[]).await.map(|_| ())
}

async fn run_create<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
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
            let base = params.len();
            let marks = (0..cells.len())
                .map(|i| dialect.mark(base + i))
                .collect::<Vec<_>>()
                .join(", ");
            groups.push(format!("({marks})"));
            params.extend(cells.iter().map(|pair| pair.1.clone()));
        }
        let sql = dialect.create(schema, head, &columns, &groups.join(", "));
        run_execute(conn, dialect, &sql, &params).await?;
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
        let id = run_insert(dialect, conn, schema.table.as_str(), &columns, &params).await?;
        out.push(Key::Int(id));
    }
    Ok(out)
}

async fn run_replace<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    schema: &Schema,
    key: &Value,
    cells: &[(Name, Value)],
) -> Result<(), StoreError> {
    let mut sets = Vec::with_capacity(cells.len());
    let mut params = Vec::with_capacity(cells.len() + 1);
    for (name, value) in cells {
        params.push(value.clone());
        sets.push(format!(
            "\"{name}\" = {}",
            dialect.mark(params.len() - 1)
        ));
    }
    params.push(key.clone());
    let sql = format!(
        "UPDATE \"{}\" SET {} WHERE \"{}\" = {}",
        schema.table,
        sets.join(", "),
        schema.key(),
        dialect.mark(params.len() - 1)
    );
    run_execute(conn, dialect, &sql, &params).await?;
    Ok(())
}

async fn run_upsert<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
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
        let created = run_create(conn, dialect, schema, batch).await?;
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
        let base = params.len();
        let marks = (0..cells.len())
            .map(|i| dialect.mark(base + i))
            .collect::<Vec<_>>()
            .join(", ");
        groups.push(format!("({marks})"));
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
    run_execute(conn, dialect, &sql, &params).await
}

async fn run_remove<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    schema: &Schema,
    key: &Value,
) -> Result<(), StoreError> {
    let sql = format!(
        "DELETE FROM \"{}\" WHERE \"{}\" = {}",
        schema.table,
        schema.key(),
        dialect.mark(0)
    );
    run_execute(conn, dialect, &sql, std::slice::from_ref(key)).await?;
    Ok(())
}

async fn run_evolve<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    schema: &Schema,
    trim: bool,
) -> Result<usize, StoreError> {
    let mut done = 0;
    run_define(conn, dialect, schema).await?;
    done += 1;
    let have = run_columns(conn, dialect, schema.table.as_str()).await?;
    for sql in rename(schema, &have) {
        run_execute(conn, dialect, &sql, &[]).await?;
        done += 1;
    }
    for sql in alter(dialect, schema, &have) {
        run_execute(conn, dialect, &sql, &[]).await?;
        done += 1;
    }
    if trim {
        for sql in drop(schema, &have) {
            run_execute(conn, dialect, &sql, &[]).await?;
            done += 1;
        }
    }
    Ok(done)
}

async fn run_mass<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    schema: &Schema,
    query: &Query,
) -> Result<Value, StoreError> {
    let mass = match query.mass {
        Some(mass) => mass,
        None => return Err(StoreError::Unsupported("mass needs a mass".into())),
    };
    let (head, kinds) = match mass {
        Mass::Count => ("COUNT(*)".to_string(), vec![ColumnKind::Integer]),
        Mass::Sum(name) => {
            let kind = kind_of(schema, name)?;
            (dialect.sum(kind, name.as_str()), vec![kind])
        }
        Mass::Mean(name) => (
            dialect.mean(kind_of(schema, name)?, name.as_str()),
            vec![ColumnKind::Real],
        ),
        Mass::Low(name) => (format!("MIN(\"{name}\")"), vec![kind_of(schema, name)?]),
        Mass::High(name) => (format!("MAX(\"{name}\")"), vec![kind_of(schema, name)?]),
    };
    let mut params = Vec::new();
    let mut sql = format!("SELECT {head} FROM \"{}\"", schema.table);
    let cond = tree(dialect, &query.tree, &mut params);
    if !cond.is_empty() {
        sql.push_str(&format!(" WHERE {cond}"));
    }
    let rows = conn
        .query_all(statement(dialect, &sql, &params))
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

async fn run_columns<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
    table: &str,
) -> Result<Vec<Column>, StoreError> {
    let rows = conn
        .query_all(Statement::from_string(
            dialect.backend(),
            dialect.introspect(table),
        ))
        .await
        .map_err(sql_err)?;
    let mut out = Vec::new();
    for row in &rows {
        let name = match row.try_get_by_index::<Option<String>>(dialect.name_at()) {
            Ok(name) => name.unwrap_or_default(),
            Err(fail) => return Err(sql_err(fail)),
        };
        let sql = match row.try_get_by_index::<Option<String>>(dialect.type_at()) {
            Ok(sql) => sql.unwrap_or_default(),
            Err(fail) => return Err(sql_err(fail)),
        };
        out.push(Column {
            name,
            kind: dialect.reflect(&sql),
        });
    }
    Ok(out)
}

async fn run_last_id<D: Dialect>(
    conn: &impl ConnectionTrait,
    dialect: &D,
) -> Result<i64, StoreError> {
    let sql = match dialect.latest() {
        Some(sql) => sql,
        None => return Err(StoreError::Unsupported("no last id".into())),
    };
    let rows = conn
        .query_all(Statement::from_string(dialect.backend(), sql.to_string()))
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

async fn run_insert<D: Dialect>(
    dialect: &D,
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
    let marks = (0..columns.len())
        .map(|i| dialect.mark(i))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("INSERT INTO \"{table}\" ({cols}) VALUES ({marks}) RETURNING \"id\"");
    let rows = conn
        .query_all(statement(dialect, &sql, values))
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

pub(crate) struct Engine<D, C> {
    dialect: D,
    conn: C,
}

impl<D, C> Engine<D, C> {
    pub(crate) fn new(dialect: D, conn: C) -> Self {
        Engine { dialect, conn }
    }
}

impl<D: Dialect, C: ConnectionTrait + TransactionTrait + Send + Sync + 'static> Store
    for Engine<D, C>
{
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        Box::pin(async move { run_execute(&self.conn, &self.dialect, &sql, &params).await })
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
            run_fetch(&self.conn, &self.dialect, &sql, &params, &kinds).await
        })
    }

    fn scan_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { run_scan(&self.conn, &self.dialect, &schema, &query).await })
    }

    fn total_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { run_total(&self.conn, &self.dialect, &schema, &query).await })
    }

    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        Box::pin(async move { run_define(&self.conn, &self.dialect, &schema).await })
    }

    fn create<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<Vec<Key>, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move { run_create(&self.conn, &self.dialect, &schema, &batch).await })
    }

    fn upsert<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move { run_upsert(&self.conn, &self.dialect, &schema, &batch).await })
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
            run_replace(&self.conn, &self.dialect, &schema, &key, &cells).await
        })
    }

    fn remove<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        let key = key.value();
        Box::pin(async move { run_remove(&self.conn, &self.dialect, &schema, &key).await })
    }

    fn evolve<'a>(
        &'a self,
        schema: &'a Schema,
        trim: bool,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        Box::pin(async move { run_evolve(&self.conn, &self.dialect, &schema, trim).await })
    }

    fn mass<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Value, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { run_mass(&self.conn, &self.dialect, &schema, &query).await })
    }

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
        let table = table.to_string();
        Box::pin(async move { run_columns(&self.conn, &self.dialect, &table).await })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move { run_last_id(&self.conn, &self.dialect).await })
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
            run_insert(&self.dialect, &self.conn, &table, &columns, &values).await
        })
    }

    fn deal<'a>(&'a self) -> BoxFuture<'a, Result<Arc<dyn Store>, StoreError>> {
        Box::pin(async move {
            let txn = self.conn.begin().await.map_err(sql_err)?;
            Ok(Arc::new(Trade {
                dialect: self.dialect.clone(),
                txn: Mutex::new(Some(txn)),
            }) as Arc<dyn Store>)
        })
    }
}

pub(crate) struct Trade<D> {
    dialect: D,
    txn: Mutex<Option<DatabaseTransaction>>,
}

impl<D: Dialect> Store for Trade<D> {
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
            run_execute(txn, &self.dialect, &sql, &params).await
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
            run_fetch(txn, &self.dialect, &sql, &params, &kinds).await
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
            run_scan(txn, &self.dialect, &schema, &query).await
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
            run_total(txn, &self.dialect, &schema, &query).await
        })
    }

    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_define(txn, &self.dialect, &schema).await
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
            run_create(txn, &self.dialect, &schema, &batch).await
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
            run_upsert(txn, &self.dialect, &schema, &batch).await
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
            run_replace(txn, &self.dialect, &schema, &key, &cells).await
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
            run_remove(txn, &self.dialect, &schema, &key).await
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
            run_evolve(txn, &self.dialect, &schema, trim).await
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
            run_mass(txn, &self.dialect, &schema, &query).await
        })
    }

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
        let table = table.to_string();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_columns(txn, &self.dialect, &table).await
        })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let txn = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            run_last_id(txn, &self.dialect).await
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
            run_insert(&self.dialect, txn, &table, &columns, &values).await
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