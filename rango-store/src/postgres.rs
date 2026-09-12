use std::sync::Arc;

use sea_orm::{Database, DbBackend};

use crate::engine::{Dialect, Engine, sql_err};
use crate::{ColumnKind, Name, Schema, Store, StoreError, Type, Value};

#[derive(Clone)]
pub struct Postgres;

impl Dialect for Postgres {
    fn backend(&self) -> DbBackend {
        DbBackend::Postgres
    }

    fn mark(&self, at: usize) -> String {
        format!("${}", at + 1)
    }

    fn sql(&self, kind: &Type) -> &'static str {
        match kind {
            Type::Id => "BIGSERIAL PRIMARY KEY",
            Type::Key => "TEXT PRIMARY KEY",
            Type::Str => "TEXT",
            Type::Int | Type::Moment => "BIGINT",
            Type::Float => "DOUBLE PRECISION",
            Type::Bool => "BIGINT",
            Type::Decimal => "TEXT",
            Type::Many => unreachable!("virtual field has no column"),
            Type::Opt(inner) => self.sql(inner),
        }
    }

    fn create(
        &self,
        schema: &Schema,
        head: &[(Name, Value)],
        columns: &str,
        groups: &str,
    ) -> String {
        let key = schema.key();
        let sets = head
            .iter()
            .filter(|pair| pair.0 != key)
            .map(|pair| format!("\"{}\" = excluded.\"{}\"", pair.0, pair.0))
            .collect::<Vec<_>>()
            .join(", ");
        if sets.is_empty() {
            format!(
                "INSERT INTO \"{}\" ({columns}) VALUES {groups} ON CONFLICT(\"{key}\") DO NOTHING",
                schema.table
            )
        } else {
            format!(
                "INSERT INTO \"{}\" ({columns}) VALUES {groups} ON CONFLICT(\"{key}\") DO UPDATE SET {sets}",
                schema.table
            )
        }
    }

    fn introspect(&self, table: &str) -> String {
        format!(
            "SELECT column_name, data_type FROM information_schema.columns WHERE table_name = '{table}' AND table_schema = 'public' ORDER BY ordinal_position"
        )
    }

    fn name_at(&self) -> usize {
        0
    }

    fn type_at(&self) -> usize {
        1
    }

    fn reflect(&self, sql: &str) -> ColumnKind {
        let sql = sql.to_uppercase();
        if sql.contains("INT") || sql.contains("BOOL") {
            ColumnKind::Integer
        } else if sql.contains("CHAR") || sql.contains("TEXT") {
            ColumnKind::Text
        } else if sql.contains("REAL") || sql.contains("FLOA") || sql.contains("DOUB") {
            ColumnKind::Real
        } else {
            ColumnKind::Text
        }
    }

    fn latest(&self) -> Option<&'static str> {
        None
    }

    fn sum(&self, kind: ColumnKind, column: &str) -> String {
        match kind {
            ColumnKind::Integer => format!("CAST(SUM(\"{column}\") AS BIGINT)"),
            _ => format!("SUM(\"{column}\")"),
        }
    }

    fn mean(&self, _kind: ColumnKind, column: &str) -> String {
        format!("CAST(AVG(\"{column}\") AS DOUBLE PRECISION)")
    }
}

pub async fn connect(url: &str) -> Result<Arc<dyn Store>, StoreError> {
    let conn = Database::connect(url).await.map_err(sql_err)?;
    Ok(Arc::new(Engine::new(Postgres, conn)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, Filter, Key, Mass, Only, Op, Order, Page, Query, Sort, Table, Tree};
    use rust_decimal::Decimal;

    async fn db() -> Option<Arc<dyn Store>> {
        let url = std::env::var("RANGO_TEST_POSTGRES").unwrap_or_default();
        if url.is_empty() {
            return None;
        }
        Some(connect(&url).await.unwrap())
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

    fn mass(mass: Mass) -> Query {
        Query {
            tree: Tree::And(Vec::new()),
            sort: Vec::new(),
            page: Page::all(),
            only: Only::All,
            mass: Some(mass),
        }
    }

    fn leaf(field: &'static str, op: Op, value: Value) -> Tree {
        Tree::Leaf(Filter {
            field: Name(field),
            op,
            value,
        })
    }

    fn one(name: &str) -> Vec<Vec<(Name, Value)>> {
        vec![vec![(Name("name"), Value::str(name))]]
    }

    async fn drop_tables(db: &Arc<dyn Store>) {
        for table in ["fw", "fk"] {
            db.execute(&format!("DROP TABLE IF EXISTS \"{table}\""), &[])
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn flows() {
        let Some(db) = db().await else {
            return;
        };
        drop_tables(&db).await;
        let schema = Schema {
            table: Table("fw"),
            fields: vec![
                Field::id(),
                Field::new("name", Type::Str),
                Field::new("age", Type::Int.optional()),
            ],
            rules: Vec::new(),
        };
        db.define(&schema).await.unwrap();
        db.execute(
            "INSERT INTO \"fw\" (name, age) VALUES ($1, $2)",
            &[Value::str("ann"), Value::int(30)],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO \"fw\" (name, age) VALUES ($1, $2)",
            &[Value::str("bob"), Value::int(40)],
        )
        .await
        .unwrap();
        db.execute(
            "INSERT INTO \"fw\" (name, age) VALUES ($1, $2)",
            &[Value::str("ann"), Value::int(31)],
        )
        .await
        .unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            db.total_query(&schema, &ask(leaf("name", Op::Eq, Value::str("ann"))))
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            db.total_query(&schema, &ask(leaf("name", Op::Like, Value::str("%nn%"))))
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            db.total_query(&schema, &ask(leaf("age", Op::More, Value::int(30))))
                .await
                .unwrap(),
            2
        );
        assert_eq!(db.mass(&schema, &mass(Mass::Count)).await.unwrap(), Value::int(3));
        assert_eq!(
            db.mass(&schema, &mass(Mass::Sum(Name("age")))).await.unwrap(),
            Value::int(101)
        );
        assert_eq!(
            db.mass(&schema, &mass(Mass::Low(Name("age")))).await.unwrap(),
            Value::int(30)
        );
        assert_eq!(
            db.mass(&schema, &mass(Mass::High(Name("age")))).await.unwrap(),
            Value::int(40)
        );
        match db.mass(&schema, &mass(Mass::Mean(Name("age")))).await.unwrap() {
            Value::Float(mean) => assert!((mean - 101.0 / 3.0).abs() < 0.001),
            _ => panic!("not a mean"),
        }
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
        assert!(rows.iter().all(|row| row.values.len() == 1));
        let keyed = Schema {
            table: Table("fk"),
            fields: vec![
                Field::key("sku"),
                Field::new("price", Type::Decimal.optional()),
            ],
            rules: Vec::new(),
        };
        db.define(&keyed).await.unwrap();
        let keys = db
            .create(
                &keyed,
                &[vec![
                    (Name("sku"), Value::str("s1")),
                    (Name("price"), Value::decimal("1.5".parse::<Decimal>().unwrap())),
                ]],
            )
            .await
            .unwrap();
        assert_eq!(keys, vec![Key::Text("s1".into())]);
        assert_eq!(
            db.upsert(
                &keyed,
                &[vec![
                    (Name("sku"), Value::str("s1")),
                    (Name("price"), Value::decimal("2.5".parse::<Decimal>().unwrap())),
                ]],
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            db.total_query(&keyed, &ask(Tree::And(Vec::new())))
                .await
                .unwrap(),
            1
        );
        let cols = db.columns("fw").await.unwrap();
        assert_eq!(
            cols.iter().map(|col| col.name.as_str()).collect::<Vec<_>>(),
            vec!["id", "name", "age"]
        );
        assert_eq!(cols[2].kind, ColumnKind::Integer);
        let id = db
            .insert(
                "fw",
                &["name".into(), "age".into()],
                &[Value::str("cid"), Value::int(50)],
            )
            .await
            .unwrap();
        assert!(id > 0);
        assert!(matches!(
            db.last_id("fw").await,
            Err(StoreError::Unsupported(_))
        ));
        db.replace(&schema, &Key::Int(id), &[(Name("age"), Value::int(51))])
            .await
            .unwrap();
        let rows = db
            .scan_query(&schema, &ask(leaf("name", Op::Eq, Value::str("cid"))))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].int(2).unwrap(), 51);
        db.remove(&schema, &Key::Int(id)).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(leaf("name", Op::Eq, Value::str("cid"))))
                .await
                .unwrap(),
            0
        );
        let tx = db.deal().await.unwrap();
        assert!(matches!(tx.deal().await, Err(StoreError::Unsupported(_))));
        tx.create(&schema, &one("trx-b")).await.unwrap();
        tx.settle(true).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(leaf("name", Op::Eq, Value::str("trx-b"))))
                .await
                .unwrap(),
            1
        );
        let tx = db.deal().await.unwrap();
        tx.create(&schema, &one("trx-a")).await.unwrap();
        tx.settle(false).await.unwrap();
        assert_eq!(
            db.total_query(&schema, &ask(leaf("name", Op::Eq, Value::str("trx-a"))))
                .await
                .unwrap(),
            0
        );
        assert!(db.columns("nope").await.unwrap().is_empty());
        drop_tables(&db).await;
    }
}