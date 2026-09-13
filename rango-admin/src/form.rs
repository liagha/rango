use rango_core::model::Field;
use rango_core::{
    Error,
    store::{Cells, Gather, Value, Widget},
};

use super::row::text;

pub(crate) fn shown(field: &Field, value: Option<&Value>) -> String {
    let label = value.and_then(|v| match v {
        Value::Str(stored) => field
            .choices
            .iter()
            .find(|choice| choice.value.as_str() == stored)
            .map(|choice| choice.label),
        _ => None,
    });
    if let Some(label) = label {
        return label.to_string();
    }
    value
        .map(|v| (field.text)(&mut Cells::new(std::slice::from_ref(v))))
        .unwrap_or_default()
}

pub(crate) fn input(field: &Field, value: Option<&Value>) -> String {
    let shown_text = match (field.load(), value) {
        (Widget::Choice, Some(Value::Str(stored))) => stored.clone(),
        (Widget::Date, Some(Value::DateTime(at))) => at.format("%Y-%m-%dT%H:%M").to_string(),
        _ => shown(field, value),
    };
    control(field, &shown_text, matches!(value, Some(Value::Bool(true))))
}

pub(crate) fn input_raw(field: &Field, raw: &str) -> String {
    control(field, raw, field.load() == Widget::Check && raw == "on")
}

pub(crate) fn locked(field: &Field, value: Option<&Value>) -> String {
    format!(
        r#"<label>{}</label><p>{}</p>"#,
        field.name,
        escape(&text(value))
    )
}

pub(crate) fn value(field: &Field, raw: Option<&String>) -> Result<Value, Error> {
    if field.many {
        return Ok(Value::Null);
    }
    let raw = raw.map(String::as_str).unwrap_or("");
    if !field.choices.is_empty()
        && !raw.is_empty()
        && !field.choices.iter().any(|choice| choice.value.as_str() == raw)
    {
        return Err(Error::BadRequest(format!(
            "{} is not a valid choice",
            field.name
        )));
    }
    if raw.is_empty() && field.load() == Widget::Check {
        return Ok(Value::int(0));
    }
    if raw.is_empty() {
        return if field.optional {
            Ok(Value::Null)
        } else {
            Err(Error::BadRequest(format!("{} is required", field.name)))
        };
    }
    let mut gather = Gather::new();
    (field.parse)(raw, &mut gather)?;
    Ok(gather.value())
}

fn control(field: &Field, value: &str, checked: bool) -> String {
    if field.id || field.many {
        return String::new();
    }
    let name = field.name;
    let label = format!(r#"<label for="admin-{name}">{name}</label>"#);
    match field.load() {
        Widget::Text | Widget::Money => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="text" value="{}">"#,
                escape(value)
            )
        }
        Widget::Int | Widget::Flt => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="number" value="{}">"#,
                escape(value)
            )
        }
        Widget::Date => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="datetime-local" value="{}">"#,
                escape(value)
            )
        }
        Widget::Check => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="checkbox"{}>"#,
                if checked { " checked" } else { "" }
            )
        }
        Widget::Choice => {
            let mut options = String::new();
            if field.optional || value.is_empty() {
                options.push_str(r#"<option value="">---------</option>"#);
            }
            for choice in field.choices {
                let picked = if choice.value.as_str() == value {
                    " selected"
                } else {
                    ""
                };
                options.push_str(&format!(
                    r#"<option value="{}"{}>{}</option>"#,
                    escape(choice.value.as_str()),
                    picked,
                    escape(choice.label)
                ));
            }
            format!(
                r#"{label}<select id="admin-{name}" name="{name}">{options}</select>"#
            )
        }
    }
}

pub(crate) fn filter_input(field: &Field, value: &str) -> String {
    if field.id || field.many {
        return String::new();
    }
    let name = field.name;
    let label = format!(r#"<label for="filter-{name}">{name}</label>"#);
    let shown = escape(value);
    match field.load() {
        Widget::Text | Widget::Money => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="text" value="{shown}">"#
            )
        }
        Widget::Int | Widget::Flt => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="number" value="{shown}">"#
            )
        }
        Widget::Date => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="date" value="{shown}">"#
            )
        }
        Widget::Check => {
            let picked = |want: &str| if shown == want { " selected" } else { "" };
            format!(
                r#"{label}<select id="filter-{name}" name="{name}"><option value="">Any</option><option value="1"{}>Yes</option><option value="0"{}>No</option></select>"#,
                picked("1"),
                picked("0")
            )
        }
        Widget::Choice => {
            let mut options = String::new();
            options.push_str(r#"<option value="">Any</option>"#);
            for choice in field.choices {
                let picked = if choice.value.as_str() == value {
                    " selected"
                } else {
                    ""
                };
                options.push_str(&format!(
                    r#"<option value="{}"{}>{}</option>"#,
                    escape(choice.value.as_str()),
                    picked,
                    escape(choice.label)
                ));
            }
            format!(
                r#"{label}<select id="filter-{name}" name="{name}">{options}</select>"#
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

#[cfg(test)]
mod tests {
    use super::*;
    use rango_core::chrono::{DateTime, NaiveDateTime, Utc};

    fn raw(text: &str) -> String {
        text.to_string()
    }

    #[test]
    fn values() {
        let text_field = Field::str("f");
        assert_eq!(
            value(&text_field, Some(&raw("hi"))).unwrap(),
            Value::str("hi")
        );
        let int_field = Field::cell::<i64>("f");
        assert_eq!(value(&int_field, Some(&raw("3"))).unwrap(), Value::int(3));
        assert!(value(&int_field, Some(&raw("x"))).is_err());
        assert!(value(&int_field, None).is_err());
        let check_field = Field::check("f");
        assert_eq!(value(&check_field, None).unwrap(), Value::int(0));
        assert_eq!(
            value(&check_field, Some(&raw("on"))).unwrap(),
            Value::int(1)
        );
        let optional = Field::cell::<String>("f").optional();
        assert_eq!(value(&optional, None).unwrap(), Value::Null);
    }

    #[test]
    fn choices() {
        const COLORS: &[rango_core::model::Choice] = &[
            rango_core::model::Choice::of("red", "Red"),
            rango_core::model::Choice::of("blue", "Blue"),
        ];
        let field = Field::str("color").choices(COLORS);
        let html = input(&field, Some(&Value::str("blue")));
        assert!(html.contains(r#"<select"#));
        assert!(html.contains(r#"value="red""#));
        assert!(html.contains(r#"value="blue" selected"#));
        assert!(html.contains("Red"));
        assert_eq!(
            value(&field, Some(&raw("red"))).unwrap(),
            Value::str("red")
        );
        assert!(value(&field, Some(&raw("purple"))).is_err());
        assert_eq!(shown(&field, Some(&Value::str("red"))), "Red");
        let filter = filter_input(&field, "red");
        assert!(filter.contains(r#"value="red" selected"#));
    }

    #[test]
    fn dates() {
        let field = Field::cell::<DateTime<Utc>>("f");
        match value(&field, Some(&raw("2026-09-09T12:30"))).unwrap() {
            Value::Int(stamp) => {
                let at = NaiveDateTime::parse_from_str("2026-09-09T12:30", "%Y-%m-%dT%H:%M")
                    .unwrap()
                    .and_utc();
                assert_eq!(stamp, at.timestamp());
            }
            _ => panic!("not an int"),
        }
        assert!(value(&field, Some(&raw("not-a-date"))).is_err());
        assert!(value(&field, None).is_err());
    }
}
