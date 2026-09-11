use rango::chrono::{DateTime, Utc};
use rango::Model;

#[derive(Clone, Model)]
#[model(table = "messages")]
pub struct Message {
    #[key]
    pub id: i64,
    pub name: String,
    pub message: String,
    pub created: DateTime<Utc>,
}