use rango::chrono::{DateTime, Utc};
use rango::decimal::Decimal;
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

    fn set_id(&mut self, id: Value) {
        if let Value::Int(id) = id {
            self.id = id;
        }
    }

    fn id(&self) -> Value {
        Value::int(self.id)
    }
}

impl AdminModel for Message {}

#[derive(Clone)]
pub struct Product {
    pub sku: String,
    pub name: String,
    pub price: Decimal,
    pub category: String,
}

impl Model for Product {
    fn table() -> &'static str {
        "products"
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::key("sku"),
            Field::new("name", Type::Str),
            Field::new("price", Type::Decimal),
            Field::new("category", Type::Str.optional()).references("categories.slug"),
        ]
    }

    fn row(&self) -> Vec<Value> {
        vec![
            Value::str(&self.sku),
            Value::str(&self.name),
            Value::decimal(self.price),
            Value::str(&self.category),
        ]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Product {
            sku: row.str(0)?,
            name: row.str(1)?,
            price: row.decimal(2)?,
            category: row.str(3).unwrap_or_default(),
        })
    }

    fn set_id(&mut self, _id: Value) {}

    fn id(&self) -> Value {
        Value::str(&self.sku)
    }
}

impl AdminModel for Product {}

#[derive(Clone)]
pub struct Category {
    pub slug: String,
    pub name: String,
}

impl Model for Category {
    fn table() -> &'static str {
        "categories"
    }

    fn fields() -> Vec<Field> {
        vec![Field::key("slug"), Field::new("name", Type::Str)]
    }

    fn row(&self) -> Vec<Value> {
        vec![Value::str(&self.slug), Value::str(&self.name)]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Category {
            slug: row.str(0)?,
            name: row.str(1)?,
        })
    }

    fn set_id(&mut self, _id: Value) {}

    fn id(&self) -> Value {
        Value::str(&self.slug)
    }
}

impl AdminModel for Category {}

pub fn schema() -> Vec<Schema> {
    vec![
        Message::schema(),
        Product::schema(),
        Category::schema(),
        rango_auth::User::schema(),
    ]
}
