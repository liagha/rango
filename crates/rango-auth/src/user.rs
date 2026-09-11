use std::sync::Arc;

use rango::{
    Error, Repository, Row, Store, StoreError, Value,
    chrono::{DateTime, Utc},
    model::{Field, Model, Name, Table, Type},
};

#[derive(Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password: String,
    pub created: DateTime<Utc>,
    pub superuser: bool,
}

impl Model for User {
    fn table() -> Table {
        Table("users")
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::new("username", Type::Str).unique(),
            Field::new("password", Type::Str),
            Field::new("created", Type::Moment),
            Field::new("superuser", Type::Bool).default(Value::Bool(false)),
        ]
    }

    fn row(&self) -> Vec<Value> {
        vec![
            Value::str(&self.username),
            Value::str(&self.password),
            Value::datetime(self.created),
            Value::bool(self.superuser),
        ]
    }

    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.int(0)?,
            username: row.str(1)?,
            password: row.str(2)?,
            created: row.datetime(3)?,
            superuser: row.bool(4).unwrap_or(false),
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

impl User {
    pub async fn register(
        store: Arc<dyn Store>,
        username: &str,
        password: &str,
        superuser: bool,
    ) -> Result<User, Error> {
        let username = username.trim();
        if username.is_empty() {
            return Err(Error::BadRequest("username is required".into()));
        }
        if password.len() < 8 {
            return Err(Error::BadRequest(
                "password must be at least 8 characters".into(),
            ));
        }
        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|fail| Error::Server(fail.to_string()))?;
        let mut user = User {
            id: 0,
            username: username.into(),
            password: hash,
            created: Utc::now(),
            superuser,
        };
        match Repository::new(store).save(&mut user).await {
            Ok(()) => Ok(user),
            Err(fail) if fail.to_string().contains("UNIQUE") => {
                Err(Error::BadRequest("username is taken".into()))
            }
            Err(fail) => Err(Error::Server(fail.to_string())),
        }
    }

    pub async fn login(
        store: Arc<dyn Store>,
        username: &str,
        password: &str,
    ) -> Result<Option<User>, Error> {
        let users = Repository::<User>::new(store)
            .filter(Name("username"), &Value::str(username))
            .await?;
        for user in users {
            if bcrypt::verify(password, &user.password).unwrap_or(false) {
                return Ok(Some(user));
            }
        }
        Ok(None)
    }
}
