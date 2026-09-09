use std::sync::Mutex;

use crate::{BoxFuture, Row, Rows, Store, StoreError, Value};

#[derive(Default)]
pub struct Memory {
    tables: Mutex<Vec<Table>>,
}

struct Table {
    name: String,
    rows: Vec<Row>,
    next_id: i64,
}

enum Query {
    All,
    Where,
    Count,
}

fn guard(table: Option<&Table>) -> Result<&Table, StoreError> {
    table.ok_or_else(|| StoreError::Sql("no table".into()))
}

fn id_of(params: &[Value]) -> Result<i64, StoreError> {
    match params.first() {
        Some(Value::Int(value)) => Ok(*value),
        _ => Err(StoreError::Value("expected id param".into())),
    }
}

fn row_id(row: &Row) -> i64 {
    match row.values.first() {
        Some(Value::Int(value)) => *value,
        _ => 0,
    }
}

fn create(sql: &str) -> Result<String, StoreError> {
    let rest = sql
        .strip_prefix("CREATE TABLE IF NOT EXISTS ")
        .ok_or_else(|| StoreError::Sql("expected create".into()))?;
    let open = rest
        .find('(')
        .ok_or_else(|| StoreError::Sql("bad create".into()))?;
    Ok(rest[..open].trim().trim_matches('"').to_string())
}

fn insert(sql: &str) -> Result<String, StoreError> {
    let rest = sql
        .strip_prefix("INSERT INTO ")
        .ok_or_else(|| StoreError::Sql("expected insert".into()))?;
    let open = rest
        .find('(')
        .ok_or_else(|| StoreError::Sql("bad insert".into()))?;
    let name = rest[..open].trim().trim_matches('"').to_string();
    Ok(name)
}

fn select(sql: &str) -> Result<(String, Query), StoreError> {
    if let Some(rest) = sql.strip_prefix("SELECT COUNT(*) FROM ") {
        return Ok((rest.trim().trim_matches('"').to_string(), Query::Count));
    }
    let rest = sql
        .strip_prefix("SELECT * FROM ")
        .ok_or_else(|| StoreError::Sql(format!("unsupported {sql}")))?;
    if let Some((head, _)) = rest.split_once(" WHERE id = ?") {
        Ok((head.trim().trim_matches('"').to_string(), Query::Where))
    } else {
        let head = rest
            .split_once(" ORDER BY id")
            .map(|(head, _)| head)
            .unwrap_or(rest);
        Ok((head.trim().trim_matches('"').to_string(), Query::All))
    }
}

fn delete(sql: &str) -> Result<String, StoreError> {
    let rest = sql
        .strip_prefix("DELETE FROM ")
        .ok_or_else(|| StoreError::Sql("expected delete".into()))?;
    let head = rest
        .split_once(" WHERE id = ?")
        .map(|(head, _)| head)
        .unwrap_or(rest);
    Ok(head.trim().trim_matches('"').to_string())
}

fn locked(tables: &Mutex<Vec<Table>>) -> Result<std::sync::MutexGuard<'_, Vec<Table>>, StoreError> {
    tables
        .lock()
        .map_err(|_| StoreError::Poison("memory tables".into()))
}

impl Store for Memory {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<usize, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        Box::pin(async move {
            let mut tables = locked(&self.tables)?;
            if sql.starts_with("CREATE TABLE IF NOT EXISTS ") {
                let name = create(&sql)?;
                if tables.iter().all(|table| table.name != name) {
                    tables.push(Table {
                        name,
                        rows: Vec::new(),
                        next_id: 0,
                    });
                }
                return Ok(0);
            }
            if sql.starts_with("INSERT INTO ") {
                let name = insert(&sql)?;
                let table = tables
                    .iter_mut()
                    .find(|table| table.name == name)
                    .ok_or_else(|| StoreError::Sql(format!("no table {name}")))?;
                table.next_id += 1;
                let mut row = Vec::with_capacity(params.len() + 1);
                row.push(Value::Int(table.next_id));
                row.extend(params);
                table.rows.push(Row { values: row });
                return Ok(1);
            }
            if sql.starts_with("DELETE FROM ") {
                let name = delete(&sql)?;
                let id = id_of(&params)?;
                let table = tables
                    .iter_mut()
                    .find(|table| table.name == name)
                    .ok_or_else(|| StoreError::Sql(format!("no table {name}")))?;
                let before = table.rows.len();
                table.rows.retain(|row| row_id(row) != id);
                return Ok(before - table.rows.len());
            }
            Err(StoreError::Sql(format!("unsupported {sql}")))
        })
    }

    fn fetch<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        Box::pin(async move {
            let tables = locked(&self.tables)?;
            let (name, query) = select(&sql)?;
            let table = guard(tables.iter().find(|table| table.name == name))?;
            match query {
                Query::Count => Ok(vec![Row {
                    values: vec![Value::Int(table.rows.len() as i64)],
                }]),
                Query::All => Ok(table.rows.clone()),
                Query::Where => {
                    let id = id_of(&params)?;
                    Ok(table
                        .rows
                        .iter()
                        .filter(|row| row_id(row) == id)
                        .cloned()
                        .collect())
                }
            }
        })
    }

    fn last_id<'a>(&'a self, table: &'a str) -> BoxFuture<'a, Result<i64, StoreError>> {
        let name = table.to_string();
        Box::pin(async move {
            let tables = locked(&self.tables)?;
            let table = guard(tables.iter().find(|table| table.name == name))?;
            Ok(table.next_id)
        })
    }
}
