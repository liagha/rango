use rango::chrono::{DateTime, Utc};
use rango::prelude::*;
use rango_admin::AdminModel;

#[derive(Clone)]
pub struct Message {
    pub id: i64,
    pub name: String,
    pub message: String,
    pub created: DateTime<Utc>,
}

impl Model for Message {
    fn table() -> &'static str {
        "messages"
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("name", Type::Str),
            Field::new("message", Type::Str),
            Field::new("created", Type::DateTime),
        ]
    }

    fn row(&self) -> Vec<Value> {
        vec![
            Value::str(&self.name),
            Value::str(&self.message),
            Value::datetime(self.created),
        ]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Message {
            id: row.int(0)?,
            name: row.str(1)?,
            message: row.str(2)?,
            created: row.datetime(3)?,
        })
    }

    fn set_id(&mut self, id: i64) {
        self.id = id;
    }

    fn id(&self) -> i64 {
        self.id
    }
}

impl AdminModel for Message {}

pub fn schema() -> Vec<String> {
    vec![Message::ddl(), rango_auth::User::ddl()]
}
