use std::{
    cmp::Ordering,
    sync::{Mutex, MutexGuard},
};

use crate::{BoxFuture, ColumnKind, Row, Rows, Store, StoreError, Value};

#[derive(Default)]
pub struct Memory {
    tables: Mutex<Vec<Table>>,
}

struct Table {
    name: String,
    columns: Vec<String>,
    rows: Vec<Row>,
    next_id: i64,
}

struct Query {
    table: String,
    count: bool,
    cond: String,
    order: Option<(String, bool)>,
    limit: Option<(usize, usize)>,
}

fn guard(table: Option<&Table>) -> Result<&Table, StoreError> {
    table.ok_or_else(|| StoreError::Sql("no table".into()))
}

fn name(text: &str) -> String {
    text.trim().trim_matches('"').to_string()
}

fn quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    out
}

fn same(have: &Value, want: &Value) -> bool {
    match (have, want) {
        (Value::Int(one), Value::Int(other)) => one == other,
        (Value::Int(one), Value::Str(other)) => other.parse::<i64>().is_ok_and(|n| one == &n),
        (Value::Str(one), Value::Int(other)) => one.parse::<i64>().is_ok_and(|n| &n == other),
        _ => have == want,
    }
}

fn rank(one: Option<&Value>, other: Option<&Value>) -> Ordering {
    match (one, other) {
        (Some(Value::Int(one)), Some(Value::Int(other))) => one.cmp(other),
        (Some(Value::Float(one)), Some(Value::Float(other))) => one.total_cmp(other),
        (Some(Value::Str(one)), Some(Value::Str(other))) => one.cmp(other),
        (Some(Value::Bool(one)), Some(Value::Bool(other))) => one.cmp(other),
        (Some(Value::DateTime(one)), Some(Value::DateTime(other))) => one.cmp(other),
        (Some(Value::Decimal(one)), Some(Value::Decimal(other))) => one.cmp(other),
        (Some(Value::Null) | None, Some(Value::Null) | None) => Ordering::Equal,
        (Some(Value::Null) | None, _) => Ordering::Greater,
        (_, Some(Value::Null) | None) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

fn sort(
    rows: &mut [Row],
    columns: &[String],
    order: Option<(String, bool)>,
) -> Result<(), StoreError> {
    let Some((column, down)) = order else {
        return Ok(());
    };
    let Some(at) = columns.iter().position(|name| name == &column) else {
        return Err(StoreError::Sql(format!("no column {column}")));
    };
    rows.sort_by(|one, other| {
        let order = rank(one.values.get(at), other.values.get(at));
        if down { order.reverse() } else { order }
    });
    Ok(())
}

fn create(sql: &str) -> Result<(String, Vec<String>), StoreError> {
    let rest = sql
        .strip_prefix("CREATE TABLE IF NOT EXISTS ")
        .ok_or_else(|| StoreError::Sql("expected create".into()))?;
    let mut names = quoted(rest);
    if names.is_empty() {
        return Err(StoreError::Sql("bad create".into()));
    }
    let table = names.remove(0);
    Ok((table, names))
}

fn insert(sql: &str) -> Result<(String, Vec<String>), StoreError> {
    let rest = sql
        .strip_prefix("INSERT INTO ")
        .ok_or_else(|| StoreError::Sql("expected insert".into()))?;
    let mut names = quoted(rest);
    if names.is_empty() {
        return Err(StoreError::Sql("bad insert".into()));
    }
    let table = names.remove(0);
    Ok((table, names))
}

fn select(sql: &str) -> Result<Query, StoreError> {
    let (rest, count) = match sql.strip_prefix("SELECT COUNT(*) FROM ") {
        Some(rest) => (rest, true),
        None => (
            sql.strip_prefix("SELECT * FROM ")
                .ok_or_else(|| StoreError::Sql(format!("unsupported {sql}")))?,
            false,
        ),
    };
    let (rest, limit) = match rest.split_once(" LIMIT ") {
        Some((head, tail)) => {
            let (number, offset) = match tail.split_once(" OFFSET ") {
                Some((number, offset)) => (
                    number
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| StoreError::Sql(format!("unsupported {sql}")))?,
                    offset
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| StoreError::Sql(format!("unsupported {sql}")))?,
                ),
                None => (
                    tail.trim()
                        .parse::<usize>()
                        .map_err(|_| StoreError::Sql(format!("unsupported {sql}")))?,
                    0,
                ),
            };
            (head, Some((number, offset)))
        }
        None => (rest, None),
    };
    let (rest, order) = match rest.split_once(" ORDER BY ") {
        Some((head, tail)) => {
            let (column, down) = match tail.strip_suffix(" DESC") {
                Some(column) => (column, true),
                None => (tail, false),
            };
            (head, Some((name(column), down)))
        }
        None => (rest, None),
    };
    let (table, cond) = match rest.split_once(" WHERE ") {
        Some((head, tail)) => (name(head), tail.to_string()),
        None => (name(rest), String::new()),
    };
    Ok(Query {
        table,
        count,
        cond,
        order,
        limit,
    })
}

struct Filter<'a> {
    text: &'a str,
    at: usize,
    params: &'a [Value],
    next: usize,
    columns: &'a [String],
    row: &'a Row,
}

fn matches(columns: &[String], row: &Row, cond: &str, params: &[Value]) -> bool {
    if cond.is_empty() {
        return true;
    }
    let mut filter = Filter {
        text: cond,
        at: 0,
        params,
        next: 0,
        columns,
        row,
    };
    filter.expr().unwrap_or(false)
}

fn blanks(filter: &mut Filter) {
    while filter.text.as_bytes().get(filter.at) == Some(&b' ') {
        filter.at += 1;
    }
}

fn word(filter: &mut Filter, want: &str) -> bool {
    blanks(filter);
    if filter.text[filter.at..].starts_with(want) {
        filter.at += want.len();
        true
    } else {
        false
    }
}

fn column(filter: &mut Filter) -> Result<String, StoreError> {
    blanks(filter);
    let rest = &filter.text[filter.at..];
    let inner = if rest.starts_with("LOWER(") {
        filter.at += 6;
        let found = column(filter)?;
        blanks(filter);
        if !word(filter, ")") {
            return Err(StoreError::Sql("bad lower".into()));
        }
        found
    } else if let Some(body) = rest.strip_prefix('"') {
        let end = body
            .find('"')
            .ok_or_else(|| StoreError::Sql("bad column".into()))?;
        let found = body[..end].to_string();
        filter.at += end + 2;
        found
    } else {
        return Err(StoreError::Sql("bad column".into()));
    };
    Ok(inner)
}

fn param(filter: &mut Filter) -> Result<Value, StoreError> {
    blanks(filter);
    if !word(filter, "?") {
        return Err(StoreError::Sql("expected param".into()));
    }
    let found = filter
        .params
        .get(filter.next)
        .cloned()
        .ok_or_else(|| StoreError::Sql("missing param".into()))?;
    filter.next += 1;
    Ok(found)
}

fn field(columns: &[String], row: &Row, name: &str) -> Option<Value> {
    columns
        .iter()
        .position(|item| item == name)
        .and_then(|at| row.values.get(at).cloned())
}

fn like(have: &str, want: &str) -> bool {
    let (have, want) = (have.to_lowercase(), want.to_lowercase());
    let body = want.trim_matches('%');
    if want.starts_with('%') && want.ends_with('%') {
        have.contains(body)
    } else if want.starts_with('%') {
        have.ends_with(body)
    } else if want.ends_with('%') {
        have.starts_with(body)
    } else {
        have == body
    }
}

fn shown(value: &Value) -> String {
    match value {
        Value::Str(found) => found.clone(),
        Value::Int(found) => found.to_string(),
        Value::Float(found) => found.to_string(),
        Value::Bool(found) => found.to_string(),
        Value::DateTime(at) => at.to_string(),
        Value::Decimal(found) => found.to_string(),
        Value::Null => String::new(),
    }
}

impl<'a> Filter<'a> {
    fn expr(&mut self) -> Result<bool, StoreError> {
        let mut found = self.term()?;
        loop {
            if word(self, "OR") {
                let other = self.term()?;
                found = found || other;
            } else {
                return Ok(found);
            }
        }
    }

    fn term(&mut self) -> Result<bool, StoreError> {
        let mut found = self.factor()?;
        loop {
            if word(self, "AND") {
                let other = self.factor()?;
                found = found && other;
            } else {
                return Ok(found);
            }
        }
    }

    fn factor(&mut self) -> Result<bool, StoreError> {
        blanks(self);
        if word(self, "(") {
            let found = self.expr()?;
            blanks(self);
            if !word(self, ")") {
                return Err(StoreError::Sql("bad group".into()));
            }
            return Ok(found);
        }
        if word(self, "1 = 0") {
            return Ok(false);
        }
        self.pred()
    }

    fn pred(&mut self) -> Result<bool, StoreError> {
        let lowered = self.text[self.at..].starts_with("LOWER(");
        let name = column(self)?;
        blanks(self);
        if word(self, "LIKE") {
            let want = param(self)?;
            let have = field(self.columns, self.row, &name).unwrap_or(Value::Null);
            let pattern = shown(&want);
            if lowered {
                return Ok(like(&shown(&have), &pattern));
            }
            let body = pattern.trim_matches('%');
            return Ok(shown(&have).contains(body));
        }
        if word(self, "IN") {
            blanks(self);
            if !word(self, "(") {
                return Err(StoreError::Sql("bad in".into()));
            }
            let mut wants = Vec::new();
            loop {
                blanks(self);
                if word(self, ")") {
                    break;
                }
                wants.push(param(self)?);
                blanks(self);
                if word(self, ",") {
                    continue;
                }
            }
            let have = field(self.columns, self.row, &name).unwrap_or(Value::Null);
            return Ok(wants.iter().any(|want| same(&have, want)));
        }
        let op = if word(self, "=") {
            0
        } else if word(self, ">=") {
            1
        } else if word(self, "<") {
            2
        } else {
            return Err(StoreError::Sql("bad op".into()));
        };
        let want = param(self)?;
        let have = field(self.columns, self.row, &name).unwrap_or(Value::Null);
        match op {
            0 => Ok(same(&have, &want)),
            1 => Ok(rank(Some(&have), Some(&want)) != Ordering::Less),
            _ => Ok(rank(Some(&have), Some(&want)) == Ordering::Less),
        }
    }
}

fn update(sql: &str) -> Result<(String, usize), StoreError> {
    let rest = sql
        .strip_prefix("UPDATE ")
        .ok_or_else(|| StoreError::Sql("expected update".into()))?;
    let (table, set) = rest
        .split_once(" SET ")
        .ok_or_else(|| StoreError::Sql("bad update".into()))?;
    let set = set
        .split_once(" WHERE ")
        .map(|(set, _)| set)
        .ok_or_else(|| StoreError::Sql("bad update".into()))?;
    Ok((name(table), set.split(", ").count()))
}

fn delete(sql: &str) -> Result<String, StoreError> {
    let rest = sql
        .strip_prefix("DELETE FROM ")
        .ok_or_else(|| StoreError::Sql("expected delete".into()))?;
    let head = rest
        .split_once(" WHERE ")
        .map(|(head, _)| head)
        .unwrap_or(rest);
    Ok(name(head))
}

fn locked(tables: &Mutex<Vec<Table>>) -> Result<MutexGuard<'_, Vec<Table>>, StoreError> {
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
                let (name, columns) = create(&sql)?;
                match tables.iter_mut().find(|table| table.name == name) {
                    Some(table) => table.columns = columns,
                    None => tables.push(Table {
                        name,
                        columns,
                        rows: Vec::new(),
                        next_id: 0,
                    }),
                }
                return Ok(0);
            }
            if sql.starts_with("INSERT INTO ") {
                let (name, columns) = insert(&sql)?;
                let table = tables
                    .iter_mut()
                    .find(|table| table.name == name)
                    .ok_or_else(|| StoreError::Sql(format!("no table {name}")))?;
                if columns.is_empty() || !params.len().is_multiple_of(columns.len()) {
                    return Err(StoreError::Value("bad params".into()));
                }
                let mut done = 0;
                for group in params.chunks(columns.len()) {
                    let mut row: Vec<Value> = Vec::with_capacity(group.len() + 1);
                    if !columns.iter().any(|column| column == "id") {
                        table.next_id += 1;
                        row.push(Value::Int(table.next_id));
                    }
                    row.extend(group.iter().cloned());
                    table.rows.push(Row { values: row });
                    done += 1;
                }
                return Ok(done);
            }
            if sql.starts_with("UPDATE ") {
                let (name, count) = update(&sql)?;
                let want = params
                    .last()
                    .ok_or_else(|| StoreError::Value("expected id param".into()))?;
                let table = tables
                    .iter_mut()
                    .find(|table| table.name == name)
                    .ok_or_else(|| StoreError::Sql(format!("no table {name}")))?;
                let mut touched = 0;
                for row in &mut table.rows {
                    if row.values.first().is_none_or(|first| !same(first, want)) {
                        continue;
                    }
                    if row.values.len() < count + 1 {
                        row.values.resize(count + 1, Value::Null);
                    }
                    for (i, value) in params.iter().take(count).enumerate() {
                        row.values[i + 1] = value.clone();
                    }
                    touched += 1;
                }
                return Ok(touched);
            }
            if sql.starts_with("DELETE FROM ") {
                let name = delete(&sql)?;
                let want = params
                    .first()
                    .ok_or_else(|| StoreError::Value("expected id param".into()))?;
                let table = tables
                    .iter_mut()
                    .find(|table| table.name == name)
                    .ok_or_else(|| StoreError::Sql(format!("no table {name}")))?;
                let before = table.rows.len();
                table
                    .rows
                    .retain(|row| row.values.first().is_none_or(|first| !same(first, want)));
                return Ok(before - table.rows.len());
            }
            Err(StoreError::Sql(format!("unsupported {sql}")))
        })
    }

    fn fetch<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [Value],
        _kinds: &'a [ColumnKind],
    ) -> BoxFuture<'a, Result<Rows, StoreError>> {
        let sql = sql.to_string();
        let params = params.to_vec();
        Box::pin(async move {
            let tables = locked(&self.tables)?;
            let query = select(&sql)?;
            let table = guard(tables.iter().find(|table| table.name == query.table))?;
            let mut rows: Vec<Row> = table
                .rows
                .iter()
                .filter(|row| matches(&table.columns, row, &query.cond, &params))
                .cloned()
                .collect();
            if query.count {
                return Ok(vec![Row {
                    values: vec![Value::Int(rows.len() as i64)],
                }]);
            }
            sort(&mut rows, &table.columns, query.order)?;
            if let Some((count, offset)) = query.limit {
                rows = rows.into_iter().skip(offset).take(count).collect();
            }
            Ok(rows)
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

    fn insert<'a>(
        &'a self,
        table: &'a str,
        columns: &'a [String],
        values: &'a [Value],
    ) -> BoxFuture<'a, Result<i64, StoreError>> {
        let name = table.to_string();
        let columns = columns.to_vec();
        let values = values.to_vec();
        Box::pin(async move {
            let mut tables = locked(&self.tables)?;
            let table = tables
                .iter_mut()
                .find(|table| table.name == name)
                .ok_or_else(|| StoreError::Sql(format!("no table {name}")))?;
            if columns.len() != values.len() {
                return Err(StoreError::Value("bad params".into()));
            }
            let mut row = Vec::with_capacity(values.len() + 1);
            if !columns.iter().any(|column| column == "id") {
                table.next_id += 1;
                row.push(Value::Int(table.next_id));
            }
            row.extend(values.iter().cloned());
            table.rows.push(Row { values: row });
            Ok(table.next_id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Memory {
        Memory::default()
    }

    #[tokio::test]
    async fn roundtrip() {
        let db = store();
        db.execute(
            "CREATE TABLE IF NOT EXISTS \"t\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"name\" TEXT)",
            &[],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO \"t\" (\"name\") VALUES (?)",
            &[Value::str("a")],
        )
        .await
        .unwrap();
        assert_eq!(db.last_id("t").await.unwrap(), 1);
        let rows = db
            .fetch("SELECT * FROM \"t\" ORDER BY \"id\"", &[], &[])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].str(1).unwrap(), "a");
        let one = db
            .fetch(
                "SELECT * FROM \"t\" WHERE \"id\" = ?",
                &[Value::int(1)],
                &[],
            )
            .await
            .unwrap();
        assert_eq!(one.len(), 1);
        let count = db
            .fetch("SELECT COUNT(*) FROM \"t\"", &[], &[])
            .await
            .unwrap();
        assert_eq!(count[0].int(0).unwrap(), 1);
    }

    #[tokio::test]
    async fn change() {
        let db = store();
        db.execute(
            "CREATE TABLE IF NOT EXISTS \"t\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"age\" INTEGER)",
            &[],
        )
        .await
        .unwrap();
        db.execute("INSERT INTO \"t\" (\"age\") VALUES (?)", &[Value::int(1)])
            .await
            .unwrap();
        let touched = db
            .execute(
                "UPDATE \"t\" SET \"age\" = ? WHERE \"id\" = ?",
                &[Value::int(2), Value::int(1)],
            )
            .await
            .unwrap();
        assert_eq!(touched, 1);
        let rows = db
            .fetch("SELECT * FROM \"t\" ORDER BY \"id\"", &[], &[])
            .await
            .unwrap();
        assert_eq!(rows[0].int(1).unwrap(), 2);
        let gone = db
            .execute("DELETE FROM \"t\" WHERE \"id\" = ?", &[Value::int(1)])
            .await
            .unwrap();
        assert_eq!(gone, 1);
        assert!(
            db.fetch("SELECT * FROM \"t\" ORDER BY \"id\"", &[], &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn missing() {
        let db = store();
        assert!(
            db.fetch("SELECT * FROM \"nope\" ORDER BY \"id\"", &[], &[])
                .await
                .is_err()
        );
        assert!(db.columns("nope").await.is_err());
    }
}
