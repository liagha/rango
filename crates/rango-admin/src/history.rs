use rango::{
    Repository, Row, StoreError, Value,
    chrono::{DateTime, Utc},
    model::{Field, Model, Type},
};
use rango_auth::Current;

use super::row::text;

#[derive(Clone)]
pub(crate) struct History {
    pub(crate) id: i64,
    pub(crate) model: String,
    pub(crate) row: String,
    pub(crate) action: Action,
    pub(crate) user: String,
    pub(crate) at: DateTime<Utc>,
}

impl Model for History {
    fn table() -> &'static str {
        "history"
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("model", Type::Str),
            Field::new("row", Type::Str),
            Field::new("action", Type::Str),
            Field::new("user", Type::Str),
            Field::new("at", Type::DateTime),
        ]
    }

    fn row(&self) -> Vec<Value> {
        vec![
            Value::str(&self.model),
            Value::str(&self.row),
            Value::str(self.action.name()),
            Value::str(&self.user),
            Value::datetime(self.at),
        ]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.int(0)?,
            model: row.str(1)?,
            row: row.str(2)?,
            action: Action::parse(&row.str(3)?)?,
            user: row.str(4)?,
            at: row.datetime(5)?,
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

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Action {
    Create,
    Edit,
    Delete,
}

impl Action {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Edit => "edit",
            Self::Delete => "delete",
        }
    }

    fn parse(raw: &str) -> Result<Self, StoreError> {
        match raw {
            "create" => Ok(Self::Create),
            "edit" => Ok(Self::Edit),
            "delete" => Ok(Self::Delete),
            _ => Err(StoreError::Value(format!("bad action {raw}"))),
        }
    }
}

pub(crate) async fn log(
    history: &Repository<History>,
    table: &'static str,
    row: &Value,
    action: Action,
    current: &Current,
) {
    let mut entry = History {
        id: 0,
        model: table.to_string(),
        row: text(Some(row)),
        action,
        user: current
            .0
            .as_ref()
            .map(|user| user.username.clone())
            .unwrap_or_default(),
        at: Utc::now(),
    };
    let _ = history.save(&mut entry).await;
}
