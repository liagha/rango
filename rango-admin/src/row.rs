use rango_core::{
    chrono::{DateTime, Utc},
    model::{Field, Model, Name},
    store::Value,
};

pub(crate) fn stamp(at: &DateTime<Utc>) -> String {
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
        Some(Value::DateTime(at)) => stamp(at),
        Some(Value::Decimal(value)) => value.to_string(),
        Some(Value::Null) | None => String::new(),
    }
}

pub(crate) fn id_of(values: &[Value], fields: &[Field]) -> String {
    fields
        .iter()
        .position(|field| field.id || field.keyed)
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
            .position(|field| field.name == *name && !field.id && !field.many)
            && !out.contains(&i)
        {
            out.push(i);
        }
    }
    out
}

/// Full row against the field list, id first. Many fields are virtual — no row()
/// slot — and pad their position with Null; keyed fields are consumed and dropped.
pub(crate) fn with_id<M: Model>(model: &M, fields: &[Field]) -> Vec<Value> {
    let mut out = vec![model.id()];
    let mut values = model.row().into_iter();
    for field in fields {
        if field.id {
            continue;
        }
        if field.many {
            out.push(Value::Null);
            continue;
        }
        let value = values.next().unwrap_or(Value::Null);
        if field.keyed {
            continue;
        }
        out.push(value);
    }
    out
}

/// Scanned values against the field list. Many fields are virtual — no scanned
/// column — and pad their position with Null; remaining columns are consumed in order.
pub(crate) fn align(values: &[Value], fields: &[Field]) -> Vec<Value> {
    let mut out = Vec::with_capacity(fields.len());
    let mut slots = values.iter();
    for field in fields {
        if field.many {
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
        Storable, StoreError,
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
            vec![Field::id(), Field::str("title"), Field::many("tags")]
        }

        fn write(&self, w: &mut dyn rango_core::Writer) {
            Storable::put(&self.title, w);
        }

        fn read(r: &mut dyn rango_core::Reader) -> Result<Self, StoreError> {
            Ok(Self {
                id: Storable::take(r)?,
                title: Storable::take(r)?,
            })
        }

        fn write_id(&self, w: &mut dyn rango_core::Writer) {
            Storable::put(&self.id, w);
        }

        fn read_id(&mut self, r: &mut dyn rango_core::Reader) -> Result<(), StoreError> {
            self.id = Storable::take(r)?;
            Ok(())
        }
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::str("title"),
            Field::many("tags"),
            Field::str("extra"),
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
        assert_eq!(
            text(Some(&Value::Decimal(rango_core::decimal::Decimal::new(
                12, 1
            )))),
            "1.2"
        );
        let at = Utc.with_ymd_and_hms(2026, 9, 9, 12, 30, 0).unwrap();
        assert_eq!(text(Some(&Value::datetime(at))), "2026-09-09 12:30");
    }

    #[test]
    fn ids() {
        let fields = vec![Field::id(), Field::str("title")];
        let values = vec![Value::int(7), Value::str("hi")];
        assert_eq!(id_of(&values, &fields), "7");
        let keyed = vec![Field::key::<String>("sku"), Field::str("name")];
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
