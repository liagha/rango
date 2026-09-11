use std::sync::Arc;

use rango::Model;
use rango::chrono::{DateTime, Utc};
use rango::decimal::Decimal;
use rango::model::{Action, Key, Name, Repository, Schema};
use rango::store::{BoxFuture, Store, StoreError};

#[derive(Clone, Model)]
#[model(table = "messages", actions = "duplicate")]
pub struct Message {
    #[key]
    pub id: i64,
    pub name: String,
    pub message: String,
    pub created: DateTime<Utc>,
}

fn duplicate() -> Action {
    Action {
        name: Name("duplicate"),
        title: "Duplicate",
        run: run_duplicate,
        logged: false,
        row: true,
    }
}

fn run_duplicate(
    store: Arc<dyn Store>,
    _schema: Schema,
    keys: Vec<Key>,
) -> BoxFuture<'static, Result<String, StoreError>> {
    Box::pin(async move {
        let repo = Repository::<Message>::new(store);
        let mut done = 0;
        for key in &keys {
            let Some(mut model) = repo.get(&key.value()).await? else {
                continue;
            };
            model.id = 0;
            repo.save(&mut model).await?;
            done += 1;
        }
        if done == 1 {
            Ok("Duplicated 1 row.".into())
        } else {
            Ok(format!("Duplicated {done} rows."))
        }
    })
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
