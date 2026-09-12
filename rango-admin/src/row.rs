use rango_core::{
    chrono::{DateTime, Utc},
    model::{Field, Model, Name, Type, many},
    store::Value,
};

pub(crate) fn when(at: &DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M").to_string()
}

pub(crate) fn cell(values: &[Value], i: usize) -> String {
    text(values.get(i))
}

pub(crate) fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Str(value)) => value.clone(),
        Some(Value::Int(value)) => value.to_string(),
        Some(Value::Float(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::DateTime(at)) => when(at),
        Some(Value::Decimal(value)) => value.to_string(),
        Some(Value::Null) | None => String::new(),
    }
}

pub(crate) fn id_of(values: &[Value], fields: &[Field]) -> String {
    fields
        .iter()
        .position(|field| matches!(field.kind.flat(), Type::Id | Type::Key))
        .and_then(|i| values.get(i))
        .map(|value| match value {
            Value::Int(id) => id.to_string(),
            Value::Str(id) => id.clone(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

pub(crate) fn locate(names: &[Name], fields: &[Field]) -> Vec<usize> {
    let mut out = Vec::new();
    for name in names {
        if let Some(i) = fields
            .iter()
            .position(|field| field.name == *name && field.kind != Type::Id && !many(&field.kind))
            && !out.contains(&i)
        {
            out.push(i);
        }
    }
    out
}

pub(crate) fn with_id<M: Model>(model: &M, fields: &[Field]) -> Vec<Value> {
    let mut out = vec![model.id()];
    let mut values = model.row().into_iter();
    for field in fields {
        if field.kind == Type::Id {
            continue;
        }
        if many(&field.kind) {
            out.push(Value::Null);
            continue;
        }
        let value = values.next().unwrap_or(Value::Null);
        if matches!(field.kind.flat(), Type::Key) {
            continue;
        }
        out.push(value);
    }
    out
}

pub(crate) fn align(values: &[Value], fields: &[Field]) -> Vec<Value> {
    let mut out = Vec::with_capacity(fields.len());
    let mut slots = values.iter();
    for field in fields {
        if many(&field.kind) {
            out.push(Value::Null);
        } else {
            out.push(slots.next().cloned().unwrap_or(Value::Null));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rango_core::{
        Row, StoreError,
        chrono::{TimeZone, Utc},
        model::Table,
    };

    #[derive(Clone)]
    struct Thread {
        id: i64,
        title: String,
    }

    impl Model for Thread {
        fn table() -> Table {
            Table("threads")
        }

        fn fields() -> Vec<Field> {
            vec![
                Field::id(),
                Field::new("title", Type::Str),
                Field::new("tags", Type::Many),
            ]
        }

        fn row(&self) -> Vec<Value> {
            vec![Value::str(&self.title)]
        }

        fn from_row(row: &Row) -> Result<Self, StoreError> {
            Ok(Self {
                id: row.int(0)?,
                title: row.str(1)?,
            })
        }

        fn set_id(&mut self, id: Value) {
            if let Value::Int(id) = id {
                self.id = id;
            }
        }

        fn id(&self) -> Value {
            Value::int(self.id)
        }
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("title", Type::Str),
            Field::new("tags", Type::Many),
            Field::new("extra", Type::Str),
        ]
    }

    #[test]
    fn texts() {
        assert_eq!(text(None), "");
        assert_eq!(text(Some(&Value::Null)), "");
        assert_eq!(text(Some(&Value::str("hi"))), "hi");
        assert_eq!(text(Some(&Value::int(7))), "7");
        assert_eq!(text(Some(&Value::bool(true))), "true");
        assert_eq!(text(Some(&Value::Float(1.5))), "1.5");
        assert_eq!(text(Some(&Value::Decimal(rango_core::decimal::Decimal::new(12, 1)))), "1.2");
        let at = Utc.with_ymd_and_hms(2026, 9, 9, 12, 30, 0).unwrap();
        assert_eq!(text(Some(&Value::datetime(at))), "2026-09-09 12:30");
    }

    #[test]
    fn ids() {
        let fields = vec![Field::id(), Field::new("title", Type::Str)];
        let values = vec![Value::int(7), Value::str("hi")];
        assert_eq!(id_of(&values, &fields), "7");
        let keyed = vec![Field::key("sku"), Field::new("name", Type::Str)];
        let products = vec![Value::str("a1"), Value::str("widget")];
        assert_eq!(id_of(&products, &keyed), "a1");
    }

    #[test]
    fn locates() {
        let all = fields();
        assert_eq!(locate(&[Name("title"), Name("tags")], &all), vec![1]);
        assert_eq!(locate(&[Name("title"), Name("title")], &all), vec![1]);
        assert_eq!(locate(&[Name("nope")], &all), Vec::<usize>::new());
    }

    #[test]
    fn with_ids() {
        let thread = Thread {
            id: 7,
            title: "hi".into(),
        };
        let all = fields();
        let out = with_id(&thread, &all);
        assert_eq!(
            out,
            vec![Value::int(7), Value::str("hi"), Value::Null, Value::Null]
        );
    }

    #[test]
    fn aligns() {
        let all = fields();
        let out = align(&[Value::int(7), Value::str("hi"), Value::int(3)], &all);
        assert_eq!(
            out,
            vec![Value::int(7), Value::str("hi"), Value::Null, Value::int(3)]
        );
        let sparse = align(&[Value::int(7)], &all);
        assert_eq!(
            sparse,
            vec![Value::int(7), Value::Null, Value::Null, Value::Null]
        );
    }
}
