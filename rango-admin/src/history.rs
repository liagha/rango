use rango_authentication::Current;
use rango_core::{
    Reader, Repository, Storable, StoreError, Value, Writer,
    chrono::{DateTime, Utc},
    model::{Field, Model, Table},
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
            Field::str("model"),
            Field::str("row"),
            Field::str("action"),
            Field::str("user"),
            Field::cell::<DateTime<Utc>>("at"),
        ]
    }

    fn write(&self, w: &mut dyn Writer) {
        Storable::put(&self.model, w);
        Storable::put(&self.row, w);
        w.str(self.action.name());
        Storable::put(&self.user, w);
        Storable::put(&self.at, w);
    }

    fn read(r: &mut dyn Reader) -> Result<Self, StoreError> {
        let id = Storable::take(r)?;
        let model = Storable::take(r)?;
        let row = Storable::take(r)?;
        let action: String = Storable::take(r)?;
        Ok(Self {
            id,
            model,
            row,
            action: Action::parse(&action)?,
            user: Storable::take(r)?,
            at: Storable::take(r)?,
        })
    }

    fn write_id(&self, w: &mut dyn Writer) {
        Storable::put(&self.id, w);
    }

    fn read_id(&mut self, r: &mut dyn Reader) -> Result<(), StoreError> {
        self.id = Storable::take(r)?;
        Ok(())
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
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-{name}.sqlite", std::process::id()));
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
