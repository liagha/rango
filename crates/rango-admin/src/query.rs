use std::{cmp::Ordering, collections::HashMap};

use rango::chrono::NaiveDate;
use rango::model::{Field, Type};
use rango::store::Value;

use super::row::text;

pub(crate) const PAGE: usize = 25;

pub(crate) fn keep(
    fields: &[Field],
    values: &[Value],
    query: &str,
    params: &HashMap<String, String>,
    find: &[usize],
) -> bool {
    if !query.is_empty() {
        let found = find
            .iter()
            .any(|&i| text(values.get(i)).to_lowercase().contains(query));
        if !found {
            return false;
        }
    }
    fields.iter().enumerate().all(|(i, field)| {
        if field.kind == Type::Id {
            return true;
        }
        match params.get(field.name) {
            Some(raw) if !raw.is_empty() => hit(field, values.get(i), raw),
            _ => true,
        }
    })
}

fn hit(field: &Field, value: Option<&Value>, raw: &str) -> bool {
    match field.kind.flat() {
        Type::Id | Type::Optional(_) => true,
        Type::Str => text(value).to_lowercase().contains(&raw.to_lowercase()),
        Type::Int => match (value, raw.parse::<i64>()) {
            (Some(Value::Int(have)), Ok(want)) => *have == want,
            _ => false,
        },
        Type::DateTime => match (value, NaiveDate::parse_from_str(raw, "%Y-%m-%d")) {
            (Some(Value::DateTime(have)), Ok(want)) => have.date_naive() == want,
            _ => false,
        },
        Type::Float => match (value, raw.parse::<f64>()) {
            (Some(Value::Float(have)), Ok(want)) => *have == want,
            _ => false,
        },
        Type::Bool => match value {
            Some(Value::Bool(have)) => *have == matches!(raw, "1" | "true" | "on" | "yes"),
            _ => false,
        },
    }
}

pub(crate) fn sort_rows(fields: &[Field], rows: &mut [Vec<Value>], sort: &str) {
    if sort.is_empty() {
        return;
    }
    let (name, down) = match sort.strip_prefix('-') {
        Some(name) => (name, true),
        None => (sort, false),
    };
    let Some(at) = fields.iter().position(|field| field.name == name) else {
        return;
    };
    rows.sort_by(|one, other| {
        let order = compare(one.get(at), other.get(at));
        if down { order.reverse() } else { order }
    });
}

fn compare(one: Option<&Value>, other: Option<&Value>) -> Ordering {
    match (one, other) {
        (Some(Value::Int(one)), Some(Value::Int(other))) => one.cmp(other),
        (Some(Value::Float(one)), Some(Value::Float(other))) => one.total_cmp(other),
        (Some(Value::Str(one)), Some(Value::Str(other))) => one.cmp(other),
        (Some(Value::Bool(one)), Some(Value::Bool(other))) => one.cmp(other),
        (Some(Value::DateTime(one)), Some(Value::DateTime(other))) => one.cmp(other),
        (Some(Value::Null) | None, Some(Value::Null) | None) => Ordering::Equal,
        (Some(Value::Null) | None, _) => Ordering::Greater,
        (_, Some(Value::Null) | None) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

pub(crate) fn encode(params: &HashMap<String, String>, skip: &[&str]) -> String {
    let mut pairs: Vec<(&String, &String)> = params
        .iter()
        .filter(|(key, _)| !skip.contains(&key.as_str()))
        .collect();
    pairs.sort();
    serde_urlencoded::to_string(pairs).unwrap_or_default()
}

pub(crate) fn href(base: &str, extra: &str) -> String {
    if base.is_empty() {
        format!("?{extra}")
    } else {
        format!("?{base}&{extra}")
    }
}
