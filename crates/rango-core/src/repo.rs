use std::{marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    model::{self, Kind, Model},
    store::{Store, StoreError, Value},
};

pub struct Repo<M = ()> {
    store: Arc<dyn Store>,
    marker: PhantomData<M>,
}

fn pairs<M: Model>(model: &M) -> Vec<(&'static str, Value)> {
    let mut values = model.row().into_iter();
    M::fields()
        .into_iter()
        .filter(|field| field.kind != Kind::Id)
        .map(|field| {
            let value = values.next().unwrap_or(Value::Null);
            let value = match field.default {
                Some(ref default) if value == Value::Null => default.clone(),
                _ => value,
            };
            (field.name, value)
        })
        .collect()
}

impl<M: Model> Repo<M> {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            marker: PhantomData,
        }
    }

    async fn ensure(&self) -> Result<(), StoreError> {
        self.store
            .execute(&model::create_table::<M>(), &[])
            .await
            .map(|_| ())
    }

    pub async fn save(&self, model: &mut M) -> Result<(), StoreError> {
        self.ensure().await?;
        let pairs = pairs(model);
        let columns: Vec<&str> = pairs.iter().map(|pair| pair.0).collect();
        let params: Vec<Value> = pairs.into_iter().map(|pair| pair.1).collect();
        let holes = vec!["?"; params.len()].join(", ");
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({holes})",
            M::table(),
            columns.join(", ")
        );
        self.store.execute(&sql, &params).await?;
        let id = self.store.last_id(M::table()).await?;
        model.set_id(id);
        Ok(())
    }

    pub async fn get(&self, id: i64) -> Result<Option<M>, StoreError> {
        self.ensure().await?;
        let sql = format!("SELECT * FROM {} WHERE id = ?", M::table());
        let rows = self.store.fetch(&sql, &[Value::int(id)]).await?;
        rows.into_iter()
            .next()
            .map(|row| M::from_row(&row))
            .transpose()
    }

    pub async fn all(&self) -> Result<Vec<M>, StoreError> {
        self.ensure().await?;
        let sql = format!("SELECT * FROM {} ORDER BY id", M::table());
        let rows = self.store.fetch(&sql, &[]).await?;
        rows.iter().map(|row| M::from_row(row)).collect()
    }

    pub async fn update(&self, model: &M) -> Result<(), StoreError> {
        self.ensure().await?;
        let mut sets = Vec::new();
        let mut params = Vec::new();
        for (name, value) in pairs(model) {
            sets.push(format!("{name} = ?"));
            params.push(value);
        }
        params.push(Value::int(model.id()));
        let sql = format!("UPDATE {} SET {} WHERE id = ?", M::table(), sets.join(", "));
        self.store.execute(&sql, &params).await?;
        Ok(())
    }

    pub async fn delete(&self, id: i64) -> Result<(), StoreError> {
        self.ensure().await?;
        let sql = format!("DELETE FROM {} WHERE id = ?", M::table());
        self.store
            .execute(&sql, &[Value::int(id)])
            .await
            .map(|_| ())
    }
}

impl<M: Model> FromRequestParts<()> for Repo<M> {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, _state: &()) -> Result<Self, Self::Rejection> {
        let store = parts
            .extensions
            .get::<Arc<dyn Store>>()
            .cloned()
            .ok_or_else(|| Error::Server("no store configured".into()))?;
        Ok(Self::new(store))
    }
}
