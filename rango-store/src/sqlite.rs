use std::{path::Path, sync::Arc};

use sea_orm::{ConnectionTrait, Database, DbBackend};

use crate::engine::{Dialect, Engine, sql_err};
use crate::{ColumnKind, Name, Schema, Store, StoreError, Type, Value};

#[derive(Clone)]
pub struct Sqlite;

impl Dialect for Sqlite {
    fn backend(&self) -> DbBackend {
        DbBackend::Sqlite
    }

    fn mark(&self, _at: usize) -> String {
        "?".into()
    }

    fn sql(&self, kind: &Type) -> &'static str {
        match kind {
            Type::Id => "INTEGER PRIMARY KEY AUTOINCREMENT",
            Type::Key => "TEXT PRIMARY KEY",
            Type::Str => "TEXT",
            Type::Int | Type::Moment => "INTEGER",
            Type::Float => "REAL",
            Type::Bool => "INTEGER",
            Type::Decimal => "TEXT",
            Type::Many => unreachable!("virtual field has no column"),
            Type::Opt(inner) => self.sql(inner),
        }
    }

    fn create(
        &self,
        schema: &Schema,
        _head: &[(Name, Value)],
        columns: &str,
        groups: &str,
    ) -> String {
        format!(
            "INSERT OR REPLACE INTO \"{}\" ({columns}) VALUES {groups}",
            schema.table
        )
    }

    fn introspect(&self, table: &str) -> String {
        format!("PRAGMA table_info(\"{table}\")")
    }

    fn name_at(&self) -> usize {
        1
    }

    fn type_at(&self) -> usize {
        2
    }

    fn reflect(&self, sql: &str) -> ColumnKind {
        let sql = sql.to_uppercase();
        if sql.contains("INT") {
            ColumnKind::Integer
        } else if sql.contains("CHAR") || sql.contains("CLOB") || sql.contains("TEXT") {
            ColumnKind::Text
        } else if sql.contains("REAL") || sql.contains("FLOA") || sql.contains("DOUB") {
            ColumnKind::Real
        } else {
            ColumnKind::Text
        }
    }

    fn latest(&self) -> Option<&'static str> {
        Some("SELECT last_insert_rowid()")
    }

    fn sum(&self, _kind: ColumnKind, column: &str) -> String {
        format!("SUM(\"{column}\")")
    }

    fn mean(&self, _kind: ColumnKind, column: &str) -> String {
        format!("AVG(\"{column}\")")
    }
}

pub async fn open(path: impl AsRef<Path>) -> Result<Arc<dyn Store>, StoreError> {
    let url = format!("sqlite://{}?mode=rwc", path.as_ref().display());
    let conn = Database::connect(&url).await.map_err(sql_err)?;
    Ok(Arc::new(Engine::new(Sqlite, conn)))
}

pub async fn open_wal(path: impl AsRef<Path>) -> Result<Arc<dyn Store>, StoreError> {
    let url = format!("sqlite://{}?mode=rwc", path.as_ref().display());
    let conn = Database::connect(&url).await.map_err(sql_err)?;
    for pragma in [
        "PRAGMA journal_mode=WAL",
        "PRAGMA synchronous=NORMAL",
        "PRAGMA busy_timeout=5000",
    ] {
        conn.execute_unprepared(pragma).await.map_err(sql_err)?;
    }
    Ok(Arc::new(Engine::new(Sqlite, conn)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{alter, ddl, drop, kinds, rename};
    use crate::{Column, Field, Filter, Key, Mass, Name, Only, Op, Order, Page, Query, Sort, Table, Tree};
    use chrono::{NaiveDate, NaiveTime};

    async fn open_db(name: &str) -> Arc<dyn Store> {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-{}.sqlite", std::process::id(), name));
        let _ = std::fs::remove_file(&path);
        open(&path).await.unwrap()
    }

    async fn store() -> Arc<dyn Store> {
        open_db("base").await
    }

    fn schema() -> Schema {
        Schema {
            table: Table("w"),
            fields: vec![
                Field::id(),
                Field::new("name", Type::Str),
                Field::new("age", Type::Int.optional()),
                Field::new("at", Type::Moment.optional()),
            ],
            rules: Vec::new(),
        }
    }

    fn col(name: &str, kind: ColumnKind) -> Column {
        Column {
            name: name.to_string(),
            kind,
        }
    }

    fn posts() -> Schema {
        Schema {
            table: Table("posts"),
            fields: vec![Field::id(), Field::new("title", Type::Str)],
            rules: Vec::new(),
        }
    }

    fn keyed() -> Schema {
        Schema {
            table: Table("products"),
            fields: vec![Field::key("sku"), Field::new("price", Type::Decimal)],
            rules: Vec::new(),
        }
    }

    #[test]
    fn renders() {
        assert_eq!(
            ddl(&Sqlite, &posts()),
            "CREATE TABLE IF NOT EXISTS \"posts\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"title\" TEXT NOT NULL)"
        );
        assert_eq!(
            ddl(&Sqlite, &keyed()),
            "CREATE TABLE IF NOT EXISTS \"products\" (\"sku\" TEXT PRIMARY KEY, \"price\" TEXT NOT NULL)"
        );
        assert_eq!(kinds(&posts()), vec![ColumnKind::Integer, ColumnKind::Text]);
    }

    #[test]
    fn alters() {
        let schema = posts();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
        ];
        assert!(alter(&Sqlite, &schema, &have).is_empty());
        let missing = vec![col("id", ColumnKind::Integer)];
        assert_eq!(alter(&Sqlite, &schema, &missing).len(), 1);
    }

    #[test]
    fn drops() {
        let schema = posts();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
            col("junk", ColumnKind::Text),
        ];
        assert_eq!(drop(&schema, &have).len(), 1);
        assert!(drop(&schema, &have[..2]).is_empty());
    }

    #[test]
    fn renames() {
        let schema = posts();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
        ];
        assert_eq!(rename(&schema, &have).len(), 1);
        assert!(alter(&Sqlite, &schema, &have).is_empty());
        assert!(drop(&schema, &have).is_empty());
        let mixed = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
            col("age", ColumnKind::Integer),
        ];
        assert_eq!(rename(&schema, &mixed).len(), 1);
        assert!(alter(&Sqlite, &schema, &mixed).is_empty());
        assert_eq!(drop(&schema, &mixed).len(), 1);
    }

    #[tokio::test]
    async fn upserts() {
        let db = open_db("upserts").await;
        let schema = Schema {
            table: Table("p"),
            fields: vec![Field::key("code"), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let rows = vec![
            vec![(Name("code"), Value::str("x")), (Name("name"), Value::str("n1"))],
            vec![(Name("code"), Value::str("x")), (Name("name"), Value::str("n2"))],
            vec![(Name("code"), Value::str("y")), (Name("name"), Value::str("m"))],
        ];
        db.create(&schema, &rows).await.unwrap();
        let query = |api| async { db.total_query(&schema, &ask(api)).await.unwrap() };
        assert_eq!(query(Tree::And(Vec::new())).await, 2);
        let row = db
            .scan_query(&schema, &ask(Tree::And(Vec::new())))
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.str(0).unwrap() == "x")
            .expect("x row");
        assert_eq!(row.str(1).unwrap(), "n2");
    }

    #[tokio::test]
    async fn upsert_merge() {
        let db = open_db("upsert_merge").await;
        let schema = Schema {
            table: Table("p2"),
            fields: vec![Field::key("code"), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let one = vec![vec![
            (Name("code"), Value::str("x")),
            (Name("name"), Value::str("n1")),
        ]];
        assert_eq!(db.upsert(&schema, &one).await.unwrap(), 1);
        let two = vec![
            vec![
                (Name("code"), Value::str("x")),
                (Name("name"), Value::str("n2")),
            ],
            vec![
                (Name("code"), Value::str("y")),
                (Name("name"), Value::str("m")),
            ],
        ];
        assert_eq!(db.upsert(&schema, &two).await.unwrap(), 2);
        let query = |api| async { db.total_query(&schema, &ask(api)).await.unwrap() };
        assert_eq!(query(Tree::And(Vec::new())).await, 2);
        let row = db
            .scan_query(&schema, &ask(Tree::And(Vec::new())))
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.str(0).unwrap() == "x")
            .expect("x row");
        assert_eq!(row.str(1).unwrap(), "n2");
    }

    fn ask(tree: Tree) -> Query {
        Query {
            tree,
            sort: Vec::new(),
            page: Page::all(),
            only: Only::All,
            mass: None,
        }
    }

    fn leaf(field: &'static str, op: Op, value: Value) -> Tree {
        Tree::Leaf(Filter {
            field: Name(field),
            op,
            value,
        })
    }

    async fn seed() -> Arc<dyn Store> {
        let db = open_db("trees").await;
        db.define(&schema()).await.unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_time(NaiveTime::MIN)
            .and_utc();
        let rows = vec![
            (Value::str("ann"), Value::int(30), Value::datetime(day)),
            (Value::str("bob"), Value::int(40), Value::Null),
            (Value::str("ann"), Value::int(30), Value::datetime(day)),
            (Value::str("zed"), Value::Null, Value::Null),
        ];
        for (name, age, at) in rows {
            db.execute(
                "INSERT INTO w (name, age, at) VALUES (?, ?, ?)",
                &[name, age, at],
            )
            .await
            .unwrap();
        }
        db
    }

    #[tokio::test]
    async fn trees() {
        let db = seed().await;
        let schema = schema();
        let total = |tree: Tree| {
            let db = db.clone();
            let schema = schema.clone();
            async move { db.total_query(&schema, &ask(tree)).await.unwrap() }
        };
        assert_eq!(total(Tree::And(Vec::new())).await, 4);
        assert_eq!(total(leaf("name", Op::Eq, Value::str("ann"))).await, 2);
        assert_eq!(total(leaf("name", Op::Like, Value::str("%ann%"))).await, 2);
        assert_eq!(
            total(Tree::Or(vec![
                leaf("name", Op::Eq, Value::str("ann")),
                leaf("age", Op::Eq, Value::int(40)),
            ]))
            .await,
            3
        );
        assert_eq!(
            total(Tree::Cut(Box::new(leaf("name", Op::Eq, Value::str("ann"))))).await,
            2
        );
        assert_eq!(total(leaf("name", Op::Ne, Value::str("bob"))).await, 3);
        assert_eq!(total(leaf("age", Op::More, Value::int(30))).await, 1);
        assert_eq!(total(leaf("age", Op::Less, Value::int(40))).await, 2);
        assert_eq!(total(leaf("age", Op::Bare, Value::Null)).await, 1);
        let day = NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_time(NaiveTime::MIN)
            .and_utc();
        assert_eq!(total(leaf("at", Op::At, Value::datetime(day))).await, 2);
        let rows = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: vec![Sort {
                        field: Name("age"),
                        order: Order::Desc,
                    }],
                    page: Page {
                        count: 2,
                        offset: 0,
                    },
                    only: Only::All,
                    mass: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].str(1).unwrap(), "bob");
        let rows = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: Vec::new(),
                    page: Page::all(),
                    only: Only::Some(vec![Name("name")]),
                    mass: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.values.len() == 1));
        assert_eq!(rows[0].str(0).unwrap(), "ann");
        let rows = db
            .scan_query(
                &schema,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: vec![Sort {
                        field: Name("name"),
                        order: Order::Asc,
                    }],
                    page: Page::all(),
                    only: Only::Lone,
                    mass: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].str(1).unwrap(), "ann");
    }

    #[tokio::test]
    async fn writes() {
        let db = open_db("writes").await;
        let schema = Schema {
            table: Table("p"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let keys = db
            .create(
                &schema,
                &[
                    vec![(Name("name"), Value::str("a"))],
                    vec![(Name("name"), Value::str("b"))],
                ],
            )
            .await
            .unwrap();
        assert_eq!(keys, vec![Key::Int(1), Key::Int(2)]);
        let keyed = Schema {
            table: Table("k"),
            fields: vec![Field::key("sku"), Field::new("price", Type::Int)],
            rules: Vec::new(),
        };
        db.define(&keyed).await.unwrap();
        let keys = db
            .create(
                &keyed,
                &[vec![
                    (Name("sku"), Value::str("s1")),
                    (Name("price"), Value::int(5)),
                ]],
            )
            .await
            .unwrap();
        assert_eq!(keys, vec![Key::Text("s1".into())]);
        db.replace(&schema, &Key::Int(1), &[(Name("name"), Value::str("a2"))])
            .await
            .unwrap();
        let rows = db
            .scan_query(&schema, &ask(Tree::And(Vec::new())))
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].str(1).unwrap(), "a2");
        db.remove(&schema, &Key::Int(2)).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn evolves() {
        let db = open_db("evolves").await;
        let one = Schema {
            table: Table("e"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        assert_eq!(db.evolve(&one, false).await.unwrap(), 1);
        assert_eq!(db.evolve(&one, false).await.unwrap(), 1);
        let two = Schema {
            table: Table("e"),
            fields: vec![
                Field::id(),
                Field::new("name", Type::Str),
                Field::new("age", Type::Int.optional()),
            ],
            rules: Vec::new(),
        };
        assert_eq!(db.evolve(&two, false).await.unwrap(), 2);
        let names = db
            .columns("e")
            .await
            .unwrap()
            .into_iter()
            .map(|col| col.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"age".to_string()));
        assert_eq!(db.evolve(&one, false).await.unwrap(), 1);
        assert_eq!(db.evolve(&one, true).await.unwrap(), 2);
        let names = db
            .columns("e")
            .await
            .unwrap()
            .into_iter()
            .map(|col| col.name)
            .collect::<Vec<_>>();
        assert!(!names.contains(&"age".to_string()));
    }

    #[tokio::test]
    async fn masses() {
        let db = open_db("masses").await;
        let schema = Schema {
            table: Table("g"),
            fields: vec![
                Field::id(),
                Field::new("name", Type::Str),
                Field::new("age", Type::Int.optional()),
            ],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        for (name, age) in [
            (Value::str("ann"), Value::int(30)),
            (Value::str("bob"), Value::int(40)),
            (Value::str("cid"), Value::Null),
        ] {
            db.execute("INSERT INTO g (name, age) VALUES (?, ?)", &[name, age])
                .await
                .unwrap();
        }
        let mass = |mass: Mass, tree: Tree| {
            let db = db.clone();
            let schema = schema.clone();
            async move {
                db.mass(
                    &schema,
                    &Query {
                        tree,
                        sort: Vec::new(),
                        page: Page::all(),
                        only: Only::All,
                        mass: Some(mass),
                    },
                )
                .await
                .unwrap()
            }
        };
        assert_eq!(
            mass(Mass::Count, Tree::And(Vec::new())).await,
            Value::int(3)
        );
        assert_eq!(
            mass(Mass::Count, leaf("name", Op::Eq, Value::str("ann"))).await,
            Value::int(1)
        );
        assert_eq!(
            mass(Mass::Sum(Name("age")), Tree::And(Vec::new())).await,
            Value::int(70)
        );
        assert_eq!(
            mass(Mass::Low(Name("age")), Tree::And(Vec::new())).await,
            Value::int(30)
        );
        assert_eq!(
            mass(Mass::High(Name("age")), Tree::And(Vec::new())).await,
            Value::int(40)
        );
        match mass(Mass::Mean(Name("age")), Tree::And(Vec::new())).await {
            Value::Float(mean) => assert!((mean - 35.0).abs() < 0.001),
            _ => panic!("not a mean"),
        }
    }

    #[tokio::test]
    async fn roundtrip() {
        let db = store().await;
        db.execute(
            "CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, age INTEGER, score REAL, flag INTEGER, at INTEGER)",
            &[],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO t (name, age, score, flag, at) VALUES (?, ?, ?, ?, ?)",
            &[
                Value::str("a"),
                Value::int(1),
                Value::float(1.5),
                Value::bool(true),
                Value::int(1700000001),
            ],
        )
        .await
        .unwrap();
        assert_eq!(db.last_id("t").await.unwrap(), 1);
        let kinds = [
            ColumnKind::Integer,
            ColumnKind::Text,
            ColumnKind::Integer,
            ColumnKind::Real,
            ColumnKind::Integer,
            ColumnKind::Integer,
        ];
        let rows = db
            .fetch("SELECT * FROM t ORDER BY id", &[], &kinds)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].str(1).unwrap(), "a");
        assert_eq!(rows[0].int(2).unwrap(), 1);
        assert_eq!(rows[0].float(3).unwrap(), 1.5);
        assert!(rows[0].bool(4).unwrap());
        assert_eq!(rows[0].datetime(5).unwrap().timestamp(), 1700000001);
        let cols = db.columns("t").await.unwrap();
        assert_eq!(
            cols.iter().map(|col| col.name.as_str()).collect::<Vec<_>>(),
            vec!["id", "name", "age", "score", "flag", "at"]
        );
        let count = db
            .fetch("SELECT COUNT(*) FROM t", &[], &[ColumnKind::Integer])
            .await
            .unwrap();
        assert_eq!(count[0].int(0).unwrap(), 1);
        let touched = db
            .execute(
                "UPDATE t SET age = ? WHERE id = ?",
                &[Value::int(2), Value::int(1)],
            )
            .await
            .unwrap();
        assert_eq!(touched, 1);
        let gone = db
            .execute("DELETE FROM t WHERE id = ?", &[Value::int(1)])
            .await
            .unwrap();
        assert_eq!(gone, 1);
    }

    #[tokio::test]
    async fn missing() {
        let db = store().await;
        assert!(db.columns("nope").await.unwrap().is_empty());
        assert!(db.fetch("SELECT * FROM nope", &[], &[]).await.is_err());
    }

    #[tokio::test]
    async fn wipes() {
        use crate::Action;
        let db = open_db("wipes").await;
        let schema = Schema {
            table: Table("w"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        let keys = db
            .create(
                &schema,
                &[
                    vec![(Name("name"), Value::str("a"))],
                    vec![(Name("name"), Value::str("b"))],
                    vec![(Name("name"), Value::str("c"))],
                ],
            )
            .await
            .unwrap();
        let deed = Action::wipe();
        assert_eq!(deed.name, Name("wipe"));
        assert!(deed.logged);
        let msg = (deed.run)(db.clone(), schema.clone(), keys[..2].to_vec())
            .await
            .unwrap();
        assert_eq!(msg, "Deleted 2 rows.");
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
        let msg = (deed.run)(db.clone(), schema, keys[2..].to_vec())
            .await
            .unwrap();
        assert_eq!(msg, "Deleted 1 row.");
    }

    fn deals() -> Schema {
        Schema {
            table: Table("deals"),
            fields: vec![Field::id(), Field::new("name", Type::Str)],
            rules: Vec::new(),
        }
    }

    fn one() -> Vec<Vec<(Name, Value)>> {
        vec![vec![(Name("name"), Value::str("a"))]]
    }

    #[tokio::test]
    async fn commits() {
        let db = open_db("commits").await;
        let schema = deals();
        db.define(&schema).await.unwrap();
        let tx = db.deal().await.unwrap();
        tx.create(&schema, &one()).await.unwrap();
        assert_eq!(
            tx.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
        tx.settle(true).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn isolates() {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-isolates.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first = open_wal(&path).await.unwrap();
        let schema = deals();
        first.define(&schema).await.unwrap();
        let tx = first.deal().await.unwrap();
        tx.create(&schema, &one()).await.unwrap();
        let second = open_wal(&path).await.unwrap();
        let ask_all = || ask(Tree::And(Vec::new()));
        assert_eq!(second.total_query(&schema, &ask_all()).await.unwrap(), 0);
        tx.settle(true).await.unwrap();
        assert_eq!(second.total_query(&schema, &ask_all()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn aborts() {
        let db = open_db("aborts").await;
        let schema = deals();
        db.define(&schema).await.unwrap();
        let tx = db.deal().await.unwrap();
        tx.create(&schema, &one()).await.unwrap();
        tx.settle(false).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn nested() {
        let db = open_db("nested").await;
        let tx = db.deal().await.unwrap();
        let err = match tx.deal().await {
            Err(err) => err,
            Ok(_) => panic!("nested deal"),
        };
        assert!(matches!(err, StoreError::Unsupported(_)));
        tx.settle(false).await.unwrap();
    }

    #[tokio::test]
    async fn abandons() {
        let path =
            std::env::temp_dir().join(format!("rango-test-{}-abandons.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = open_wal(&path).await.unwrap();
        let schema = deals();
        db.define(&schema).await.unwrap();
        {
            let tx = db.deal().await.unwrap();
            tx.create(&schema, &one()).await.unwrap();
        }
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            0
        );
    }
}