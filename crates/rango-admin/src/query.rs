use std::collections::HashMap;

use rango::chrono::{NaiveDate, NaiveTime};
use rango::model::{Field, Type};
use rango::store::Value;

pub(crate) const PAGE: usize = 25;

pub(crate) fn cond(
    fields: &[Field],
    params: &HashMap<String, String>,
    query: &str,
    find: &[usize],
    related: &HashMap<usize, Vec<String>>,
) -> (String, Vec<Value>) {
    let mut parts = Vec::new();
    let mut values = Vec::new();
    for field in fields.iter() {
        if field.kind == Type::Id {
            continue;
        }
        let raw = params.get(field.name).map(String::as_str).unwrap_or("");
        if raw.is_empty() {
            continue;
        }
        match field.kind.flat() {
            Type::Str | Type::Key => {
                parts.push(format!("LOWER(\"{}\") LIKE ?", field.name));
                values.push(Value::str(format!("%{}%", raw.to_lowercase())));
            }
            Type::Decimal => match raw.parse::<rango::decimal::Decimal>() {
                Ok(number) => {
                    parts.push(format!("\"{}\" = ?", field.name));
                    values.push(Value::decimal(number));
                }
                Err(_) => return ("1 = 0".into(), Vec::new()),
            },
            Type::Int => match raw.parse::<i64>() {
                Ok(number) => {
                    parts.push(format!("\"{}\" = ?", field.name));
                    values.push(Value::int(number));
                }
                Err(_) => return ("1 = 0".into(), Vec::new()),
            },
            Type::Float => match raw.parse::<f64>() {
                Ok(number) => {
                    parts.push(format!("\"{}\" = ?", field.name));
                    values.push(Value::float(number));
                }
                Err(_) => return ("1 = 0".into(), Vec::new()),
            },
            Type::Bool => {
                parts.push(format!("\"{}\" = ?", field.name));
                values.push(Value::bool(matches!(raw, "1" | "true" | "on" | "yes")));
            }
            Type::DateTime => match NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
                Ok(day) => {
                    let start = day.and_time(NaiveTime::MIN).and_utc();
                    let end = day
                        .succ_opt()
                        .map(|next| next.and_time(NaiveTime::MIN).and_utc());
                    match end {
                        Some(end) => {
                            parts.push(format!(
                                "\"{}\" >= ? AND \"{}\" < ?",
                                field.name, field.name
                            ));
                            values.push(Value::datetime(start));
                            values.push(Value::datetime(end));
                        }
                        None => return ("1 = 0".into(), Vec::new()),
                    }
                }
                Err(_) => return ("1 = 0".into(), Vec::new()),
            },
            _ => {}
        }
    }
    if !query.is_empty() {
        let mut search = Vec::new();
        let mut terms = Vec::new();
        for &i in find {
            search.push(format!("LOWER(\"{}\") LIKE ?", fields[i].name));
            terms.push(Value::str(format!("%{query}%")));
        }
        for (i, ids) in related {
            if ids.is_empty() {
                continue;
            }
            let marks = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            search.push(format!("\"{}\" IN ({marks})", fields[*i].name));
            for id in ids {
                terms.push(Value::str(id));
            }
        }
        if !search.is_empty() {
            parts.push(format!("({})", search.join(" OR ")));
            values.extend(terms);
        }
    }
    (parts.join(" AND "), values)
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

pub(crate) fn here(params: &HashMap<String, String>) -> String {
    let qs = encode(params, &[]);
    if qs.is_empty() {
        String::new()
    } else {
        format!("?{qs}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("name", Type::Str),
            Field::new("age", Type::Int),
        ]
    }

    #[test]
    fn search() {
        let fields = fields();
        let params = HashMap::new();
        let (clause, terms) = cond(&fields, &params, "ann", &[1], &HashMap::new());
        assert!(clause.contains("LIKE"));
        assert_eq!(terms, vec![Value::str("%ann%")]);
        let (clause, _) = cond(&fields, &params, "", &[1], &HashMap::new());
        assert!(clause.is_empty());
    }

    #[test]
    fn filter() {
        let fields = fields();
        let mut params = HashMap::new();
        params.insert("name".into(), "an".into());
        let (clause, terms) = cond(&fields, &params, "", &[1], &HashMap::new());
        assert!(clause.contains("LIKE"));
        assert_eq!(terms, vec![Value::str("%an%")]);
        params.insert("age".into(), "30".into());
        let (clause, terms) = cond(&fields, &params, "", &[1], &HashMap::new());
        assert!(clause.contains("AND"));
        assert_eq!(terms.len(), 2);
        params.insert("age".into(), "bad".into());
        let (clause, _) = cond(&fields, &params, "", &[1], &HashMap::new());
        assert_eq!(clause, "1 = 0");
    }

    #[test]
    fn links() {
        let mut params = HashMap::new();
        params.insert("q".into(), "x".into());
        params.insert("page".into(), "2".into());
        assert_eq!(encode(&params, &["page"]), "q=x");
        assert_eq!(href("", "page=2"), "?page=2");
        assert_eq!(href("q=x", "page=2"), "?q=x&page=2");
    }
}
