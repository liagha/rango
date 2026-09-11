use std::collections::HashMap;

use rango_core::chrono::{NaiveDate, NaiveTime};
use rango_core::model::{Field, Filter, Op, Tree, Type};
use rango_core::store::Value;

pub(crate) const PAGE: usize = 25;

pub(crate) fn tree(
    fields: &[Field],
    params: &HashMap<String, String>,
    text: &str,
    find: &[usize],
    related: &HashMap<usize, Vec<String>>,
) -> Tree {
    let mut parts = Vec::new();
    for field in fields.iter() {
        if field.kind == Type::Id {
            continue;
        }
        let raw = params
            .get(field.name.as_str())
            .map(String::as_str)
            .unwrap_or("");
        if raw.is_empty() {
            continue;
        }
        match field.kind.flat() {
            Type::Str | Type::Key => parts.push(Tree::Leaf(Filter {
                field: field.name,
                op: Op::Like,
                value: Value::str(format!("%{}%", raw.to_lowercase())),
            })),
            Type::Decimal => match raw.parse::<rango_core::decimal::Decimal>() {
                Ok(number) => parts.push(Tree::Leaf(Filter {
                    field: field.name,
                    op: Op::Eq,
                    value: Value::decimal(number),
                })),
                Err(_) => return Tree::Or(Vec::new()),
            },
            Type::Int => match raw.parse::<i64>() {
                Ok(number) => parts.push(Tree::Leaf(Filter {
                    field: field.name,
                    op: Op::Eq,
                    value: Value::int(number),
                })),
                Err(_) => return Tree::Or(Vec::new()),
            },
            Type::Float => match raw.parse::<f64>() {
                Ok(number) => parts.push(Tree::Leaf(Filter {
                    field: field.name,
                    op: Op::Eq,
                    value: Value::float(number),
                })),
                Err(_) => return Tree::Or(Vec::new()),
            },
            Type::Bool => parts.push(Tree::Leaf(Filter {
                field: field.name,
                op: Op::Eq,
                value: Value::bool(matches!(raw, "1" | "true" | "on" | "yes")),
            })),
            Type::Moment => match NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
                Ok(day) => parts.push(Tree::Leaf(Filter {
                    field: field.name,
                    op: Op::At,
                    value: Value::datetime(day.and_time(NaiveTime::MIN).and_utc()),
                })),
                Err(_) => return Tree::Or(Vec::new()),
            },
            _ => {}
        }
    }
    if !text.is_empty() {
        let mut search = Vec::new();
        for &i in find {
            search.push(Tree::Leaf(Filter {
                field: fields[i].name,
                op: Op::Like,
                value: Value::str(format!("%{text}%")),
            }));
        }
        for (i, ids) in related {
            if ids.is_empty() {
                continue;
            }
            let mut ors = Vec::new();
            for id in ids {
                ors.push(Tree::Leaf(Filter {
                    field: fields[*i].name,
                    op: Op::Eq,
                    value: Value::str(id),
                }));
            }
            search.push(Tree::Or(ors));
        }
        if !search.is_empty() {
            parts.push(Tree::Or(search));
        }
    }
    Tree::And(parts)
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
        assert_eq!(
            tree(&fields, &params, "ann", &[1], &HashMap::new()),
            Tree::And(vec![Tree::Or(vec![Tree::Leaf(Filter {
                field: fields[1].name,
                op: Op::Like,
                value: Value::str("%ann%"),
            })])])
        );
        assert_eq!(
            tree(&fields, &params, "", &[1], &HashMap::new()),
            Tree::And(Vec::new())
        );
    }

    #[test]
    fn filter() {
        let fields = fields();
        let mut params = HashMap::new();
        params.insert("name".into(), "an".into());
        assert_eq!(
            tree(&fields, &params, "", &[1], &HashMap::new()),
            Tree::And(vec![Tree::Leaf(Filter {
                field: fields[1].name,
                op: Op::Like,
                value: Value::str("%an%"),
            })])
        );
        params.insert("age".into(), "30".into());
        assert_eq!(
            tree(&fields, &params, "", &[1], &HashMap::new()),
            Tree::And(vec![
                Tree::Leaf(Filter {
                    field: fields[1].name,
                    op: Op::Like,
                    value: Value::str("%an%"),
                }),
                Tree::Leaf(Filter {
                    field: fields[2].name,
                    op: Op::Eq,
                    value: Value::int(30),
                }),
            ])
        );
        params.insert("age".into(), "bad".into());
        assert_eq!(
            tree(&fields, &params, "", &[1], &HashMap::new()),
            Tree::Or(Vec::new())
        );
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
