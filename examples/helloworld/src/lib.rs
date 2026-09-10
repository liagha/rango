use rango::Model;
use rango::chrono::{DateTime, Utc};
use rango::decimal::Decimal;
use rango::prelude::*;

#[derive(Clone, Model)]
#[model(table = "messages")]
pub struct Message {
    #[key]
    pub id: i64,
    pub name: String,
    pub message: String,
    pub created: DateTime<Utc>,
}

#[derive(Clone, Model)]
#[model(table = "products")]
pub struct Product {
    #[key]
    pub sku: String,
    pub name: String,
    pub price: Decimal,
    #[references("categories.slug")]
    pub category: String,
}

#[derive(Clone, Model)]
#[model(table = "categories")]
pub struct Category {
    #[key]
    pub slug: String,
    pub name: String,
}

pub fn schema() -> Vec<Schema> {
    vec![
        Message::schema(),
        Product::schema(),
        Category::schema(),
        rango_auth::User::schema(),
        rango_admin::history(),
    ]
}
