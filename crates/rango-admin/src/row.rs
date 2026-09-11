use rango::{
    chrono::{DateTime, Utc},
    model::{Field, Model, Name, Type},
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
            .position(|field| field.name == *name && field.kind != Type::Id)
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
        let value = values.next().unwrap_or(Value::Null);
        if matches!(field.kind.flat(), Type::Key) {
            continue;
        }
        out.push(value);
    }
    out
}
