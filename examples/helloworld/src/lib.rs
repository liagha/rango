use rango::model;
use rango::prelude::*;
use rango_admin::AdminModel;

#[derive(Clone)]
pub struct Message {
    pub id: i64,
    pub name: String,
    pub message: String,
    pub created: i64,
}

impl Model for Message {
    fn table() -> &'static str {
        "messages"
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("name", Kind::Str),
            Field::new("message", Kind::Str),
            Field::new("created", Kind::DateTime),
        ]
    }

    fn row(&self) -> Vec<Value> {
        vec![
            Value::str(&self.name),
            Value::str(&self.message),
            Value::int(self.created),
        ]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Message {
            id: row.int(0)?,
            name: row.str(1)?,
            message: row.str(2)?,
            created: row.int(3)?,
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
    vec![
        model::create_table::<Message>(),
        model::create_table::<rango_auth::User>(),
    ]
}
