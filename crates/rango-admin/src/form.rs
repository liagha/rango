use rango::chrono::NaiveDateTime;
use rango::model::{Field, Type};
use rango::{Error, store::Value};

use super::row::text;

pub(crate) fn input(field: &Field, value: Option<&Value>) -> String {
    let shown = match (field.kind.flat(), value) {
        (Type::DateTime, Some(Value::DateTime(at))) => at.format("%Y-%m-%dT%H:%M").to_string(),
        _ => text(value),
    };
    control(field, &shown, matches!(value, Some(Value::Bool(true))))
}

pub(crate) fn input_raw(field: &Field, raw: &str) -> String {
    control(
        field,
        raw,
        matches!(field.kind.flat(), Type::Bool) && raw == "on",
    )
}

pub(crate) fn locked(field: &Field, value: Option<&Value>) -> String {
    format!(
        r#"<label>{}</label><p>{}</p>"#,
        field.name,
        escape(&text(value))
    )
}

pub(crate) fn value(field: &Field, raw: Option<&String>) -> Result<Value, Error> {
    let kind = field.kind.flat();
    let raw = raw.map(String::as_str).unwrap_or("");
    if raw.is_empty() && matches!(kind, Type::Bool) {
        return Ok(Value::bool(false));
    }
    if raw.is_empty() {
        return if field.kind.is_optional() {
            Ok(Value::Null)
        } else {
            Err(Error::BadRequest(format!("{} is required", field.name)))
        };
    }
    match kind {
        Type::Id | Type::Optional(_) => Ok(Value::Null),
        Type::Str => Ok(Value::str(raw)),
        Type::Int => raw
            .parse::<i64>()
            .map(Value::int)
            .map_err(|_| bad(field, "an integer")),
        Type::DateTime => NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
            .map(|at| Value::datetime(at.and_utc()))
            .map_err(|_| bad(field, "a date and time")),
        Type::Float => raw
            .parse::<f64>()
            .map(Value::float)
            .map_err(|_| bad(field, "a number")),
        Type::Bool => Ok(Value::bool(raw == "on")),
    }
}

fn control(field: &Field, value: &str, checked: bool) -> String {
    let name = field.name;
    let label = format!(r#"<label for="admin-{name}">{name}</label>"#);
    match field.kind.flat() {
        Type::Id | Type::Optional(_) => String::new(),
        Type::Str => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="text" value="{}">"#,
                escape(value)
            )
        }
        Type::Int | Type::Float => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="number" value="{}">"#,
                escape(value)
            )
        }
        Type::DateTime => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="datetime-local" value="{}">"#,
                escape(value)
            )
        }
        Type::Bool => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="checkbox"{}>"#,
                if checked { " checked" } else { "" }
            )
        }
    }
}

fn bad(field: &Field, want: &str) -> Error {
    Error::BadRequest(format!("{} must be {want}", field.name))
}

pub(crate) fn filter_input(field: &Field, value: &str) -> String {
    let name = field.name;
    let label = format!(r#"<label for="filter-{name}">{name}</label>"#);
    let value = escape(value);
    match field.kind.flat() {
        Type::Id | Type::Optional(_) => String::new(),
        Type::Str => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="text" value="{value}">"#
            )
        }
        Type::Int | Type::Float => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="number" value="{value}">"#
            )
        }
        Type::DateTime => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="date" value="{value}">"#
            )
        }
        Type::Bool => {
            let picked = |want: &str| if value == want { " selected" } else { "" };
            format!(
                r#"{label}<select id="filter-{name}" name="{name}"><option value="">Any</option><option value="1"{}>Yes</option><option value="0"{}>No</option></select>"#,
                picked("1"),
                picked("0")
            )
        }
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
