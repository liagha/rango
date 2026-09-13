use std::{collections::HashMap, sync::Arc};

use chrono::{NaiveTime, TimeDelta};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, QueryResult, Statement,
    TransactionTrait, Value as SeaValue,
};
use tokio::sync::Mutex;

use crate::{
    BoxFuture, Column, ColumnKind, Field, Filter, Key, Mass, Name, Only, Op, Policy, Query, Row,
    Rows, Rule, Schema, Store, StoreError, Tree, Value,
};

/// SQL dialect: backend grammar, schema rendering, and row decoding.
pub(crate) trait Dialect: Clone + Send + Sync + 'static {
    fn backend(&self) -> DbBackend;

    fn mark(&self, at: usize) -> String;

    fn sql(&self, dtype: &str) -> &'static str;

    fn primary(&self, auto: bool, dtype: &str) -> String {
        match (self.backend(), auto) {
            (DbBackend::Sqlite, true) => "INTEGER PRIMARY KEY AUTOINCREMENT".into(),
            (DbBackend::Postgres, true) => "BIGSERIAL PRIMARY KEY".into(),
            _ => format!("{} PRIMARY KEY", self.sql(dtype)),
        }
    }

    fn column(&self, field: &Field) -> String {
        let mut base = if field.id {
            self.primary(true, field.dtype)
        } else if field.keyed {
            self.primary(false, field.dtype)
        } else {
            let mut text = self.sql(field.dtype).to_string();
            if field.unique {
                text.push_str(" UNIQUE");
            }
            if !field.optional {
                text.push_str(" NOT NULL");
            }
            if field.default.is_some() {
                text.push_str(&format!(" DEFAULT {}", field.initial().literal()));
            }
            text
        };
        if let Some((table, column)) = field.reference() {
            let policy = match field.on_delete {
                Policy::Cascade => " ON DELETE CASCADE",
                Policy::Protect => " ON DELETE RESTRICT",
                Policy::Set => " ON DELETE SET NULL",
                Policy::Nothing => "",
            };
            base.push_str(&format!(" REFERENCES \"{table}\"(\"{column}\"){policy}"));
        }
        format!("\"{}\" {}", field.name, base)
    }

    fn ddl(&self, schema: &Schema) -> String {
        let mut parts = schema
            .fields
            .iter()
            .filter(|field| !field.many)
            .map(|field| self.column(field))
            .collect::<Vec<_>>();
        for rule in &schema.rules {
            let constraint = match rule {
                Rule::Unique(columns) => {
                    let names = columns
                        .iter()
                        .map(|name| format!("\"{name}\""))
                        .collect::<Vec<_>>();
                    format!("UNIQUE ({})", names.join(", "))
                }
                Rule::Check(expr) => format!("CHECK ({expr})"),
            };
            parts.push(constraint);
        }
        format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            schema.table,
            parts.join(", ")
        )
    }

    fn alter(&self, schema: &Schema, have: &[Column]) -> Vec<String> {
        let moved = schema.moved(have);
        let mut out = Vec::new();
        for field in &schema.fields {
            if field.id || field.many {
                continue;
            }
            let known = have.iter().any(|col| col.name == field.name.as_str());
            let renamed = moved.iter().any(|(_, name)| name == field.name.as_str());
            if !known && !renamed {
                out.push(format!(
                    "ALTER TABLE \"{}\" ADD COLUMN {}",
                    schema.table,
                    self.column(field)
                ));
            }
        }
        out
    }

    fn leaf(
        &self,
        paths: &HashMap<Name, (String, Vec<Value>)>,
        filter: &Filter,
        params: &mut Vec<Value>,
    ) -> String {
        if let Some((fragment, extra)) = paths.get(&filter.field) {
            let base = params.len();
            params.extend(extra.iter().cloned());
            let mark = self.mark(base);
            return fragment.replacen("{mark}", &mark, 1);
        }
        match filter.op {
            Op::Eq => {
                params.push(filter.value.clone());
                format!("\"{}\" = {}", filter.field, self.mark(params.len() - 1))
            }
            Op::Ne => {
                params.push(filter.value.clone());
                format!("\"{}\" != {}", filter.field, self.mark(params.len() - 1))
            }
            Op::More => {
                params.push(filter.value.clone());
                format!("\"{}\" > {}", filter.field, self.mark(params.len() - 1))
            }
            Op::Less => {
                params.push(filter.value.clone());
                format!("\"{}\" < {}", filter.field, self.mark(params.len() - 1))
            }
            Op::Like => {
                params.push(filter.value.clone());
                format!(
                    "LOWER(\"{}\") LIKE {}",
                    filter.field,
                    self.mark(params.len() - 1)
                )
            }
            Op::Bare => format!("\"{}\" IS NULL", filter.field),
            Op::At => match &filter.value {
                Value::DateTime(at) => {
                    let start = at.date_naive().and_time(NaiveTime::MIN).and_utc();
                    let end = start + TimeDelta::days(1);
                    params.push(Value::datetime(start));
                    let lo = self.mark(params.len() - 1);
                    params.push(Value::datetime(end));
                    let hi = self.mark(params.len() - 1);
                    format!("\"{}\" >= {lo} AND \"{}\" < {hi}", filter.field, filter.field)
                }
                _ => "1 = 0".into(),
            },
        }
    }

    fn tree(
        &self,
        paths: &HashMap<Name, (String, Vec<Value>)>,
        node: &Tree,
        params: &mut Vec<Value>,
    ) -> String {
        match node {
            Tree::Leaf(filter) => self.leaf(paths, filter, params),
            Tree::And(parts) => {
                let mut out = Vec::new();
                for part in parts {
                    let cond = self.tree(paths, part, params);
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
                for part in parts {
                    let cond = self.tree(paths, part, params);
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
                let cond = self.tree(paths, inner, params);
                if cond.is_empty() {
                    "1 = 1".into()
                } else {
                    format!("NOT ({cond})")
                }
            }
        }
    }

    fn create(
        &self,
        schema: &Schema,
        head: &[(Name, Value)],
        columns: &str,
        groups: &str,
    ) -> String {
        let key = schema.key();
        let sets = head
            .iter()
            .filter(|pair| pair.0 != key)
            .map(|pair| format!("\"{}\" = excluded.\"{}\"", pair.0, pair.0))
            .collect::<Vec<_>>()
            .join(", ");
        if sets.is_empty() {
            format!(
                "INSERT INTO \"{}\" ({columns}) VALUES {groups} ON CONFLICT(\"{key}\") DO NOTHING",
                schema.table
            )
        } else {
            format!(
                "INSERT INTO \"{}\" ({columns}) VALUES {groups} ON CONFLICT(\"{key}\") DO UPDATE SET {sets}",
                schema.table
            )
        }
    }

    fn introspect(&self, table: &str) -> String;

    fn shape(&self, row: &QueryResult) -> Result<(String, ColumnKind), DbErr>;

    fn fks(&self, table: &str) -> String;

    fn foreign(&self, row: &QueryResult) -> Option<String>;

    fn latest(&self) -> Option<&'static str>;

    fn sum(&self, kind: ColumnKind, column: &str) -> String;

    fn mean(&self, kind: ColumnKind, column: &str) -> String;
}

impl Tree {
    fn leaves<'a>(&'a self, out: &mut Vec<&'a Filter>) {
        match self {
            Tree::Leaf(filter) => {
                if filter.field.as_str().contains("__") {
                    out.push(filter);
                }
            }
            Tree::And(parts) | Tree::Or(parts) => {
                for part in parts {
                    part.leaves(out);
                }
            }
            Tree::Cut(inner) => inner.leaves(out),
        }
    }
}

/// Runs statements over a single connection.
pub(crate) struct Runner<C> {
    conn: C,
}

impl<C> Runner<C> {
    pub(crate) fn new(conn: C) -> Self {
        Runner { conn }
    }
}

impl<C: ConnectionTrait> Runner<C> {
    fn binds(values: &[Value]) -> Vec<SeaValue> {
        values
            .iter()
            .map(|value| match value {
                Value::Null => SeaValue::String(None),
                Value::Int(v) => SeaValue::BigInt(Some(*v)),
                Value::Float(v) => SeaValue::Double(Some(*v)),
                Value::Str(v) => SeaValue::String(Some(Box::new(v.clone()))),
                Value::Bool(v) => SeaValue::BigInt(Some(*v as i64)),
                Value::DateTime(v) => SeaValue::BigInt(Some(v.timestamp())),
                Value::Decimal(v) => SeaValue::String(Some(Box::new(v.to_string()))),
            })
            .collect()
    }

    fn statement<D: Dialect>(dialect: &D, sql: &str, params: &[Value]) -> Statement {
        Statement::from_sql_and_values(dialect.backend(), sql, Self::binds(params))
    }

    fn row(row: &QueryResult, kinds: &[ColumnKind]) -> Result<Row, StoreError> {
        let mut values = Vec::with_capacity(kinds.len());
        for (i, kind) in kinds.iter().enumerate() {
            let value = match kind {
                ColumnKind::Integer => match row.try_get_by_index::<Option<i64>>(i) {
                    Ok(Some(v)) => Value::int(v),
                    Ok(None) => Value::Null,
                    Err(err) => return Err(StoreError::from(err)),
                },
                ColumnKind::Real => match row.try_get_by_index::<Option<f64>>(i) {
                    Ok(Some(v)) => Value::float(v),
                    Ok(None) => Value::Null,
                    Err(err) => return Err(StoreError::from(err)),
                },
                ColumnKind::Text => match row.try_get_by_index::<Option<String>>(i) {
                    Ok(Some(v)) => Value::str(v),
                    Ok(None) => Value::Null,
                    Err(err) => return Err(StoreError::from(err)),
                },
            };
            values.push(value);
        }
        Ok(Row { values })
    }

    async fn execute<D: Dialect>(
        &self,
        dialect: &D,
        sql: &str,
        params: &[Value],
    ) -> Result<usize, StoreError> {
        self.conn
            .execute(Self::statement(dialect, sql, params))
            .await
            .map(|result| result.rows_affected() as usize)
            .map_err(StoreError::from)
    }

    async fn fetch<D: Dialect>(
        &self,
        dialect: &D,
        sql: &str,
        params: &[Value],
        kinds: &[ColumnKind],
    ) -> Result<Rows, StoreError> {
        let rows = self
            .conn
            .query_all(Self::statement(dialect, sql, params))
            .await
            .map_err(StoreError::from)?;
        rows.iter().map(|row| Self::row(row, kinds)).collect()
    }

    async fn paths<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        query: &Query,
    ) -> Result<HashMap<Name, (String, Vec<Value>)>, StoreError> {
        let mut seen = Vec::new();
        query.tree.leaves(&mut seen);
        let mut paths = HashMap::new();
        for filter in seen {
            let Some((fk, sub)) = filter.field.as_str().split_once("__") else {
                continue;
            };
            let field = schema
                .fields
                .iter()
                .find(|field| field.name.as_str() == fk)
                .ok_or_else(|| StoreError::Value(format!("unknown field {fk}")))?;
            let Some((table, column)) = field.reference() else {
                return Err(StoreError::Value(format!("{fk} needs a reference")));
            };
            let sql = dialect.introspect(table.as_str());
            let rows = self
                .conn
                .query_all(Self::statement(dialect, &sql, &[]))
                .await
                .map_err(StoreError::from)?;
            let has = rows
                .iter()
                .any(|row| matches!(dialect.shape(row), Ok((name, _)) if name == sub));
            if !has {
                return Err(StoreError::Value(format!("unknown column {sub}")));
            }
            let op = match filter.op {
                Op::Eq => "=",
                Op::Ne => "!=",
                Op::More => ">",
                Op::Less => "<",
                Op::Like => "LIKE",
                _ => "=",
            };
            let fragment = format!(
                "\"{fk}\" IN (SELECT \"{column}\" FROM \"{table}\" WHERE \"{sub}\" {op} {{mark}})"
            );
            paths.insert(filter.field, (fragment, vec![filter.value.clone()]));
        }
        Ok(paths)
    }

    async fn scan<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        query: &Query,
    ) -> Result<Rows, StoreError> {
        if query.mass.is_some() {
            return Err(StoreError::Unsupported("mass needs mass()".into()));
        }
        let (head, kinds) = match &query.only {
            Only::All | Only::Lone => ("SELECT *".to_string(), schema.kinds()),
            Only::Some(names) if names.is_empty() => ("SELECT *".to_string(), schema.kinds()),
            Only::Some(names) => {
                let mut kinds = Vec::with_capacity(names.len());
                for name in names {
                    kinds.push(schema.kind(*name)?);
                }
                let columns = names
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                (format!("SELECT {columns}"), kinds)
            }
        };
        let mut sql = format!("{head} FROM \"{}\"", schema.table);
        let paths = self.paths(dialect, schema, query).await?;
        let mut params = Vec::new();
        let cond = dialect.tree(&paths, &query.tree, &mut params);
        if !cond.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&cond);
        }
        sql.push_str(" ORDER BY ");
        sql.push_str(&schema.order(&query.sort));
        match &query.only {
            Only::Lone => sql.push_str(" LIMIT 1"),
            _ => {
                if query.page.count > 0 {
                    sql.push_str(&format!(
                        " LIMIT {} OFFSET {}",
                        query.page.count, query.page.offset
                    ));
                }
            }
        }
        let rows = self
            .conn
            .query_all(Self::statement(dialect, &sql, &params))
            .await
            .map_err(StoreError::from)?;
        rows.iter().map(|row| Self::row(row, &kinds)).collect()
    }

    async fn total<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        query: &Query,
    ) -> Result<usize, StoreError> {
        if query.mass.is_some() {
            return Err(StoreError::Unsupported("mass needs mass()".into()));
        }
        let paths = self.paths(dialect, schema, query).await?;
        let mut params = Vec::new();
        let cond = dialect.tree(&paths, &query.tree, &mut params);
        let mut sql = format!("SELECT COUNT(*) FROM \"{}\"", schema.table);
        if !cond.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&cond);
        }
        let rows = self
            .conn
            .query_all(Self::statement(dialect, &sql, &params))
            .await
            .map_err(StoreError::from)?;
        match rows
            .first()
            .and_then(|row| row.try_get_by_index::<Option<i64>>(0).ok().flatten())
        {
            Some(found) => Ok(found as usize),
            None => Err(StoreError::Value("no total".into())),
        }
    }

    async fn define<D: Dialect>(&self, dialect: &D, schema: &Schema) -> Result<(), StoreError> {
        self.execute(dialect, &dialect.ddl(schema), &[]).await.map(|_| ())
    }

    async fn create<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        batch: &[Vec<(Name, Value)>],
    ) -> Result<Vec<Key>, StoreError> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let keyed = schema.fields.iter().any(|field| field.keyed && !field.id);
        if keyed {
            let head = &batch[0];
            let columns = head
                .iter()
                .map(|(name, _)| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let mut params = Vec::new();
            let mut groups = Vec::new();
            for row in batch {
                let base = params.len();
                let marks = (0..row.len())
                    .map(|at| dialect.mark(base + at))
                    .collect::<Vec<_>>()
                    .join(", ");
                groups.push(format!("({marks})"));
                for (_, value) in row {
                    params.push(value.clone());
                }
            }
            let sql = dialect.create(schema, head, &columns, &groups.join(", "));
            self.execute(dialect, &sql, &params).await?;
            let id = schema.key();
            let mut keys = Vec::with_capacity(batch.len());
            for row in batch {
                let value = row
                    .iter()
                    .find(|(name, _)| *name == id)
                    .map(|(_, value)| value)
                    .ok_or_else(|| StoreError::Value("missing key".into()))?;
                keys.push(Key::of(value)?);
            }
            Ok(keys)
        } else {
            let mut keys = Vec::with_capacity(batch.len());
            for row in batch {
                let columns = row
                    .iter()
                    .map(|(name, _)| name.as_str().to_string())
                    .collect::<Vec<_>>();
                let values = row.iter().map(|(_, value)| value.clone()).collect::<Vec<_>>();
                keys.push(Key::Int(
                    self.insert(dialect, schema.table.as_str(), &columns, &values).await?,
                ));
            }
            Ok(keys)
        }
    }

    async fn upsert<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        batch: &[Vec<(Name, Value)>],
    ) -> Result<usize, StoreError> {
        if batch.is_empty() {
            return Ok(0);
        }
        let keyed = schema.fields.iter().any(|field| field.keyed && !field.id);
        if !keyed {
            return Ok(self.create(dialect, schema, batch).await?.len());
        }
        let head = &batch[0];
        let columns = head
            .iter()
            .map(|(name, _)| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut params = Vec::new();
        let mut groups = Vec::new();
        for row in batch {
            let base = params.len();
            let marks = (0..row.len())
                .map(|at| dialect.mark(base + at))
                .collect::<Vec<_>>()
                .join(", ");
            groups.push(format!("({marks})"));
            for (_, value) in row {
                params.push(value.clone());
            }
        }
        let sql = dialect.create(schema, head, &columns, &groups.join(", "));
        self.execute(dialect, &sql, &params).await
    }

    async fn replace<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        key: &Value,
        cells: &[(Name, Value)],
    ) -> Result<(), StoreError> {
        let mut params = Vec::new();
        let mut sets = Vec::new();
        for (name, value) in cells {
            params.push(value.clone());
            sets.push(format!("\"{name}\" = {}", dialect.mark(params.len() - 1)));
        }
        params.push(key.clone());
        let sql = format!(
            "UPDATE \"{}\" SET {} WHERE \"{}\" = {}",
            schema.table,
            sets.join(", "),
            schema.key(),
            dialect.mark(params.len() - 1)
        );
        self.execute(dialect, &sql, &params).await.map(|_| ())
    }

    async fn remove<D: Dialect>(
        &self,
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
        self.execute(dialect, &sql, std::slice::from_ref(key))
            .await
            .map(|_| ())
    }

    async fn evolve<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        trim: bool,
    ) -> Result<usize, StoreError> {
        let mut done = 0;
        done += usize::from(self.define(dialect, schema).await.is_ok());
        let have = self.columns(dialect, schema.table.as_str()).await?;
        let refs = self.refs(dialect, schema.table.as_str()).await?;
        for sql in schema.rename(&have, &refs) {
            self.execute(dialect, &sql, &[]).await?;
            done += 1;
        }
        for sql in dialect.alter(schema, &have) {
            self.execute(dialect, &sql, &[]).await?;
            done += 1;
        }
        if trim {
            for sql in schema.drop(&have, &refs) {
                self.execute(dialect, &sql, &[]).await?;
                done += 1;
            }
        }
        Ok(done)
    }

    async fn mass<D: Dialect>(
        &self,
        dialect: &D,
        schema: &Schema,
        query: &Query,
    ) -> Result<Value, StoreError> {
        let (hint, kinds) = match &query.mass {
            Some(Mass::Count) => ("COUNT(*)".to_string(), vec![ColumnKind::Integer]),
            Some(Mass::Sum(name)) => {
                let kind = schema.kind(*name)?;
                (dialect.sum(kind, name.as_str()), vec![kind])
            }
            Some(Mass::Mean(name)) => {
                let kind = schema.kind(*name)?;
                (dialect.mean(kind, name.as_str()), vec![ColumnKind::Real])
            }
            Some(Mass::Low(name)) => (format!("MIN(\"{name}\")"), vec![schema.kind(*name)?]),
            Some(Mass::High(name)) => (format!("MAX(\"{name}\")"), vec![schema.kind(*name)?]),
            None => return Err(StoreError::Unsupported("mass needs mass()".into())),
        };
        let paths = self.paths(dialect, schema, query).await?;
        let mut params = Vec::new();
        let cond = dialect.tree(&paths, &query.tree, &mut params);
        let mut sql = format!("SELECT {hint} FROM \"{}\"", schema.table);
        if !cond.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&cond);
        }
        let rows = self.fetch(dialect, &sql, &params, &kinds).await?;
        Ok(rows.first().and_then(|row| row.get(0).cloned()).unwrap_or(Value::Null))
    }

    async fn refs<D: Dialect>(
        &self,
        dialect: &D,
        table: &str,
    ) -> Result<Vec<String>, StoreError> {
        let sql = dialect.fks(table);
        let rows = self
            .conn
            .query_all(Statement::from_string(dialect.backend(), sql))
            .await
            .map_err(StoreError::from)?;
        let mut out = Vec::new();
        for row in &rows {
            if let Some(name) = dialect.foreign(row)
                && !name.is_empty()
                && !out.iter().any(|have| have == &name)
            {
                out.push(name);
            }
        }
        Ok(out)
    }

    async fn columns<D: Dialect>(
        &self,
        dialect: &D,
        table: &str,
    ) -> Result<Vec<Column>, StoreError> {
        let sql = dialect.introspect(table);
        let rows = self
            .conn
            .query_all(Statement::from_string(dialect.backend(), sql))
            .await
            .map_err(StoreError::from)?;
        rows.iter()
            .map(|row| {
                let (name, kind) = dialect.shape(row).map_err(StoreError::from)?;
                Ok(Column { name, kind })
            })
            .collect()
    }

    async fn last_id<D: Dialect>(&self, dialect: &D) -> Result<i64, StoreError> {
        let Some(latest) = dialect.latest() else {
            return Err(StoreError::Unsupported("no last id".into()));
        };
        let rows = self
            .conn
            .query_all(Statement::from_string(dialect.backend(), latest.to_string()))
            .await
            .map_err(StoreError::from)?;
        match rows
            .first()
            .and_then(|row| row.try_get_by_index::<Option<i64>>(0).ok().flatten())
        {
            Some(id) => Ok(id),
            None => Err(StoreError::Value("no last id".into())),
        }
    }

    async fn insert<D: Dialect>(
        &self,
        dialect: &D,
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
            .map(|at| dialect.mark(at))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("INSERT INTO \"{table}\" ({cols}) VALUES ({marks}) RETURNING \"id\"");
        let rows = self
            .conn
            .query_all(Self::statement(dialect, &sql, values))
            .await
            .map_err(StoreError::from)?;
        match rows
            .first()
            .and_then(|row| row.try_get_by_index::<Option<i64>>(0).ok().flatten())
        {
            Some(id) => Ok(id),
            None => Err(StoreError::Value("no last id".into())),
        }
    }
}

/// A grounded store: dialect and connection owned together.
pub(crate) struct Engine<D, C> {
    dialect: D,
    runner: Runner<C>,
}

impl<D, C> Engine<D, C> {
    pub(crate) fn new(dialect: D, conn: C) -> Self {
        Engine {
            dialect,
            runner: Runner::new(conn),
        }
    }
}

impl<D: Dialect, C: ConnectionTrait + TransactionTrait + Send + Sync + 'static> Store for Engine<D, C> {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        Box::pin(async move { self.runner.execute(&self.dialect, &sql, &params).await })
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
            self.runner.fetch(&self.dialect, &sql, &params, &kinds).await
        })
    }

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
        let table = table.to_string();
        Box::pin(async move { self.runner.columns(&self.dialect, &table).await })
    }

    fn scan_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { self.runner.scan(&self.dialect, &schema, &query).await })
    }

    fn total_query<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { self.runner.total(&self.dialect, &schema, &query).await })
    }

    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        Box::pin(async move { self.runner.define(&self.dialect, &schema).await })
    }

    fn create<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<Vec<Key>, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move { self.runner.create(&self.dialect, &schema, &batch).await })
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
            self.runner.replace(&self.dialect, &schema, &key, &cells).await
        })
    }

    fn upsert<'a>(
        &'a self,
        schema: &'a Schema,
        batch: &'a [Vec<(Name, Value)>],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        let batch = batch.to_vec();
        Box::pin(async move { self.runner.upsert(&self.dialect, &schema, &batch).await })
    }

    fn remove<'a>(
        &'a self,
        schema: &'a Schema,
        key: &'a Key,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        let key = key.value();
        Box::pin(async move { self.runner.remove(&self.dialect, &schema, &key).await })
    }

    fn evolve<'a>(
        &'a self,
        schema: &'a Schema,
        drop: bool,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        Box::pin(async move { self.runner.evolve(&self.dialect, &schema, drop).await })
    }

    fn mass<'a>(
        &'a self,
        schema: &'a Schema,
        query: &'a Query,
    ) -> BoxFuture<'a, Result<Value, StoreError>> {
        let schema = schema.clone();
        let query = query.clone();
        Box::pin(async move { self.runner.mass(&self.dialect, &schema, &query).await })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move { self.runner.last_id(&self.dialect).await })
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
            self.runner
                .insert(&self.dialect, &table, &columns, &values)
                .await
        })
    }

    fn deal<'a>(&'a self) -> BoxFuture<'a, Result<Arc<dyn Store>, StoreError>> {
        Box::pin(async move {
            let txn = self.runner.conn.begin().await.map_err(StoreError::from)?;
            Ok(Arc::new(Trade {
                dialect: self.dialect.clone(),
                txn: Mutex::new(Some(Runner::new(txn))),
            }) as Arc<dyn Store>)
        })
    }
}

/// A transaction wrapped as a standalone store.
pub(crate) struct Trade<D> {
    dialect: D,
    txn: Mutex<Option<Runner<DatabaseTransaction>>>,
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.execute(&self.dialect, &sql, &params).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.fetch(&self.dialect, &sql, &params, &kinds).await
        })
    }

    fn columns<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<Vec<Column>, StoreError>> {
        let table = table.to_string();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.columns(&self.dialect, &table).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.scan(&self.dialect, &schema, &query).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.total(&self.dialect, &schema, &query).await
        })
    }

    fn define<'a>(&'a self, schema: &'a Schema) -> BoxFuture<'a, Result<(), StoreError>> {
        let schema = schema.clone();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.define(&self.dialect, &schema).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.create(&self.dialect, &schema, &batch).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.replace(&self.dialect, &schema, &key, &cells).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.upsert(&self.dialect, &schema, &batch).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.remove(&self.dialect, &schema, &key).await
        })
    }

    fn evolve<'a>(
        &'a self,
        schema: &'a Schema,
        drop: bool,
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let schema = schema.clone();
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.evolve(&self.dialect, &schema, drop).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.mass(&self.dialect, &schema, &query).await
        })
    }

    fn last_id<'a>(&'a self, _table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        Box::pin(async move {
            let guard = self.txn.lock().await;
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.last_id(&self.dialect).await
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
            let runner = guard
                .as_ref()
                .ok_or_else(|| StoreError::Value("settled deal".into()))?;
            runner.insert(&self.dialect, &table, &columns, &values).await
        })
    }

    fn deal<'a>(&'a self) -> BoxFuture<'a, Result<Arc<dyn Store>, StoreError>> {
        Box::pin(async { Err(StoreError::Unsupported("nested deal".into())) })
    }

    fn settle(self: Arc<Self>, commit: bool) -> BoxFuture<'static, Result<(), StoreError>> {
        Box::pin(async move {
            let runner = {
                let mut guard = self.txn.lock().await;
                guard
                    .take()
                    .ok_or_else(|| StoreError::Value("settled deal".into()))?
            };
            if commit {
                runner.conn.commit().await.map_err(StoreError::from)
            } else {
                runner.conn.rollback().await.map_err(StoreError::from)
            }
        })
    }
}