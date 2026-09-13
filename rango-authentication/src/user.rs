use std::sync::Arc;

use rango_core::{
    Error, Reader, Repository, Storable, Store, StoreError, Value, Writer,
    chrono::{DateTime, Utc},
    model::{Field, Model, Name, Table},
};

/// A registered account persisted in the `users` repository.
#[derive(Clone)]
pub struct User {
    /// Store-assigned unique id.
    pub id: i64,
    /// Unique login name.
    pub username: String,
    /// Bcrypt password hash.
    pub password: String,
    /// Account creation timestamp.
    pub created: DateTime<Utc>,
    /// Whether the user has superuser rights.
    pub superuser: bool,
}

impl Model for User {
    fn table() -> Table {
        Table("users")
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::id(),
            Field::str("username").unique(),
            Field::str("password"),
            Field::cell::<DateTime<Utc>>("created"),
            Field::check("superuser").default_value(false),
        ]
    }

    fn write(&self, w: &mut dyn Writer) {
        Storable::put(&self.username, w);
        Storable::put(&self.password, w);
        Storable::put(&self.created, w);
        Storable::put(&self.superuser, w);
    }

    fn read(r: &mut dyn Reader) -> Result<Self, StoreError> {
        Ok(Self {
            id: Storable::take(r)?,
            username: Storable::take(r)?,
            password: Storable::take(r)?,
            created: Storable::take(r)?,
            superuser: Storable::take(r)?,
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

impl User {
    /// Validates and saves a new user, returning an error on bad input or a taken username.
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

    /// Verifies a username and password, returning the matching user or `None`.
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn open(name: &str) -> Arc<dyn Store> {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-{name}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        rango_core::store::sqlite::open(&path).await.unwrap()
    }

    #[tokio::test]
    async fn register_checks() {
        let store = open("register").await;
        assert!(matches!(
            User::register(store.clone(), "  ", "password123", false).await,
            Err(Error::BadRequest(ref message)) if message == "username is required"
        ));
        assert!(matches!(
            User::register(store.clone(), "bob", "short", false).await,
            Err(Error::BadRequest(ref message)) if message == "password must be at least 8 characters"
        ));
        let user = User::register(store, "alice", "password123", false)
            .await
            .unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.id, 1);
    }

    #[tokio::test]
    async fn register_taken() {
        let store = open("taken").await;
        User::register(store.clone(), "alice", "password123", false)
            .await
            .unwrap();
        assert!(matches!(
            User::register(store, "alice", "password123", false).await,
            Err(Error::BadRequest(ref message)) if message == "username is taken"
        ));
    }

    #[tokio::test]
    async fn login_roundtrip() {
        let store = open("login").await;
        User::register(store.clone(), "alice", "password123", true)
            .await
            .unwrap();
        let found = User::login(store.clone(), "alice", "password123")
            .await
            .unwrap()
            .unwrap();
        assert!(found.superuser);
        assert_eq!(found.id, 1);
        assert!(
            User::login(store.clone(), "alice", "wrongpass")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            User::login(store, "nobody", "password123")
                .await
                .unwrap()
                .is_none()
        );
    }
}
