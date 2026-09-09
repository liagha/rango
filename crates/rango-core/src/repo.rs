use std::{marker::PhantomData, sync::Arc};

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    error::Error,
    model::{self, Kind, Model},
    store::{Store, StoreError, Value},
};

pub struct Repo<M = ()> {
    pub store: Arc<dyn Store>,
    marker: PhantomData<M>,
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

    pub async fn sync(&self) -> Result<(), StoreError> {
        self.ensure().await
    }

    pub async fn save(&self, model: &mut M) -> Result<(), StoreError> {
        self.ensure().await?;
        let mut values = model.row().into_iter();
        let mut columns = Vec::new();
        let mut params = Vec::new();
        for field in M::fields() {
            if field.kind == Kind::Id {
                continue;
            }
            let value = values.next().unwrap_or(Value::Null);
            let value = match field.default {
                Some(ref default) if value == Value::Null => default.clone(),
                _ => value,
            };
            columns.push(field.name);
            params.push(value);
        }
        let columns = columns.join(", ");
        let holes = vec!["?"; params.len()].join(", ");
        let sql = format!("INSERT INTO {} ({columns}) VALUES ({holes})", M::table());
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
        let fields = M::fields();
        let mut values = model.row().into_iter();
        let mut sets = Vec::new();
        let mut params = Vec::new();
        for field in &fields {
            if field.kind == Kind::Id {
                continue;
            }
            let value = values.next().unwrap_or(Value::Null);
            let value = match field.default {
                Some(ref default) if value == Value::Null => default.clone(),
                _ => value,
            };
            sets.push(format!("{} = ?", field.name));
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

    pub async fn count(&self) -> Result<usize, StoreError> {
        self.ensure().await?;
        let sql = format!("SELECT COUNT(*) FROM {}", M::table());
        let rows = self.store.fetch(&sql, &[]).await?;
        match rows.first().and_then(|row| row.values.first()) {
            Some(Value::Int(count)) => Ok(*count as usize),
            _ => Err(StoreError::Value("bad count".into())),
        }
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
