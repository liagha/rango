use rango_authentication::Current;
use rango_core::{
    Repository, Row, StoreError, Value,
    chrono::{DateTime, Utc},
    model::{Field, Model, Table, Type},
};

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
    fn table() -> Table {
        Table("history")
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("model", Type::Str),
            Field::new("row", Type::Str),
            Field::new("action", Type::Str),
            Field::new("user", Type::Str),
            Field::new("at", Type::Moment),
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

#[derive(Clone, Copy, PartialEq, Debug)]
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
    table: Table,
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn open(name: &str) -> std::sync::Arc<dyn rango_core::Store> {
        let path = std::env::temp_dir().join(format!(
            "rango-test-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        rango_core::store::sqlite::open(&path).await.unwrap()
    }

    #[test]
    fn actions() {
        assert_eq!(Action::Create.name(), "create");
        assert_eq!(Action::Edit.name(), "edit");
        assert_eq!(Action::Delete.name(), "delete");
        assert_eq!(Action::parse("create").unwrap(), Action::Create);
        assert_eq!(Action::parse("edit").unwrap(), Action::Edit);
        assert_eq!(Action::parse("delete").unwrap(), Action::Delete);
        assert!(matches!(
            Action::parse("junk"),
            Err(StoreError::Value(ref message)) if message == "bad action junk"
        ));
    }

    #[tokio::test]
    async fn logs() {
        let store = open("history").await;
        let repo = Repository::<History>::new(store);
        log(
            &repo,
            Table("posts"),
            &Value::int(7),
            Action::Delete,
            &Current(None),
        )
        .await;
        log(
            &repo,
            Table("posts"),
            &Value::int(3),
            Action::Create,
            &Current(None),
        )
        .await;
        let entries = repo.all().await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model, "posts");
        assert_eq!(entries[0].row, "7");
        assert_eq!(entries[0].action, Action::Delete);
        assert_eq!(entries[0].user, "");
        assert_eq!(entries[1].row, "3");
        assert_eq!(entries[1].action, Action::Create);
    }
}
