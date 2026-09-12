# Model reference

## Model

The `Model` trait is implemented automatically by `#[derive(Clone, Model)]`:

```rust
use rango::Model;

#[derive(Clone, Model)]
#[model(table = "posts")]
pub struct Post {
    #[key]
    pub id: i64,
    pub title: String,
}
```

Generated methods:

| Method         | Description                                             |
|----------------|---------------------------------------------------------|
| `table()`      | Table name, e.g. `Table("posts")`                       |
| `fields()`     | Field definitions                                       |
| `row(&self)`   | Values for storage (primary key excluded)               |
| `from_row(row)`| Build a model from a `Row`                              |
| `id(&self)`    | Primary key as a `Value`                                |
| `set_id(&mut self, id)` | Assign the key after an insert                  |
| `schema()`     | Full `Schema` (table + fields)                          |
| `columns()`    | Column names, excluding the key and `Many` fields       |
| `search()`     | `Str` fields, used by the admin search                  |
| `readonly()`   | Fields the admin must not edit (empty by default)       |
| `actions()`    | Admin actions; includes `wipe` by default               |

### Attributes

| Attribute                              | Where    | Effect                               |
|----------------------------------------|----------|--------------------------------------|
| `#[model(table = "posts", actions = "duplicate")]` | struct | Table name and admin actions |
| `#[key]`                               | field    | Primary key                          |
| `#[references("table.column")]`        | field    | Foreign-key link                     |
| `#[via("join.mine.theirs")]`           | `Vec` field | Many-to-many through a join table |
| `#[default(expr)]`                     | field    | Default value used when null         |

## Types

| Store type | Meaning                |
|------------|------------------------|
| `Id`       | Auto-increment integer key |
| `Key`      | Text primary key       |
| `Str`      | Text                   |
| `Int`      | Integer                |
| `Float`    | Floating point         |
| `Bool`     | Boolean (0/1)          |
| `Moment`   | Timestamp (unix)       |
| `Decimal`  | Base-10 decimal, stored as text |
| `Many`     | Many-to-many (virtual, needs `via`) |
| `Opt(T)`   | Nullable column        |

Rust-to-store mapping in the derive:

| Rust type                                 | Store type |
|-------------------------------------------|------------|
| `i64`, `i32`, `i16`, `i8`, `u64`, `u32`, `u16`, `u8`, `isize`, `usize` | Int |
| `f64`, `f32`                              | Float      |
| `bool`                                    | Bool       |
| `String`, `str`                           | Str        |
| `DateTime<Utc>`                           | Moment     |
| `Decimal`                                 | Decimal    |

## Fields

| Builder                          | Description                              |
|----------------------------------|------------------------------------------|
| `Field::id()`                    | `Id` field named `id`                    |
| `Field::key("sku")`              | `Key` field                              |
| `Field::new("title", Type::Str)` | Regular field                            |
| `.optional()`                    | Wrap the type in `Opt`                   |
| `.unique()`                      | Unique constraint                        |
| `.indexed()`                     | Index                                    |
| `.default(value)`                | Default                                  |
| `.references("table.col")`       | Foreign-key link                         |

`schemas().key()` returns the name of the first `Id` or `Key` field,
falling back to `"id"` when neither exists.

## Queries

### Filters

```rust
use rango::{Filter, Op, Tree, Value};

let leaf = Tree::Leaf(Filter {
    field: Name("title"),
    op: Op::Like,
    value: Value::str("%rango%"),
});

let tree = Tree::And(vec![
    Tree::Leaf(Filter { field: Name("draft"), op: Op::Eq, value: Value::bool(false) }),
    Tree::Leaf(Filter { field: Name("views"), op: Op::More, value: Value::int(100) }),
]);
```

`Op` variants: `Eq`, `Ne`, `Less`, `More`, `At` (exact word match),
`Like` (SQL `LIKE`), `Bare` (raw suffix). Trees combine with
`And`, `Or`, and `Cut` (negation): `Tree::Leaf`, `Tree::And(Vec<Tree>)`,
`Tree::Or(Vec<Tree>)`, `Tree::Cut(Box<Tree>)`.

### Sorting

```rust
use rango::{Order, Sort};

Sort { field: Name("title"), order: Order::Asc }
Sort { field: Name("title"), order: Order::Desc }

Sort::parse("-title", &schema)   // leading "-" means descending
```

An unknown field in `Sort::parse` falls back to the schema key.

### Pagination

```rust
use rango::Page;

Page { count: 20, offset: 0 }   // LIMIT 20 OFFSET 0
Page::all()                      // no limit
```

### Projection

```rust
use rango::Only;

Only::All                                  // all columns
Only::Some(vec![Name("title"), Name("body")]) // listed columns
Only::Lone                                 // expect a single row
```

### Aggregation

```rust
use rango::{Mass, Query};

let result = store.mass(&schema, &Query {
    tree: Tree::And(Vec::new()),
    sort: Vec::new(),
    page: Page::all(),
    only: Only::All,
    mass: Some(Mass::Count),
}).await?;
```

| Mass                | Result                                  |
|---------------------|-----------------------------------------|
| `Count`             | Row count (`Value::Int`)                |
| `Sum(Name("col"))`  | Sum of an `Int` column (`Value::Int`)   |
| `Mean(Name("col"))` | Average (`Value::Float`)                |
| `Low(Name("col"))`  | Minimum                                  |
| `High(Name("col"))` | Maximum                                  |

`Low`/`High` return `Value::Int` for integer columns and `Value::Float` for
floating columns.

### Query struct

```rust
Query {
    tree: Tree::And(Vec::new()),   // WHERE
    sort: Vec::new(),              // ORDER BY
    page: Page::all(),             // LIMIT / OFFSET
    only: Only::All,               // column selection
    mass: None,                    // aggregation
}
```

## Keys

```rust
use rango::Key;

let key = Key::parse("42", &schema);        // Key::Int(42) or Key::Text("42")
let key = Key::of(&Value::int(42));          // Key::Int(42)
let key = Key::of(&Value::str("abc"));       // Key::Text("abc")
```

`Key::parse` produces `Key::Int` when the schema key is an integer and the
string parses; it always produces `Key::Text` for text keys.

## Value and Row

`Value` is one of `Null`, `Int(i64)`, `Float(f64)`, `Str(String)`,
`Bool(bool)`, `DateTime(DateTime<Utc>)`, `Decimal(Decimal)`. Construct them
with `Value::int`, `Value::float`, `Value::str`, `Value::bool`,
`Value::datetime`, `Value::decimal`.

`Row` wraps `Vec<Value>` and offers index-based accessors:
`int(i)`, `float(i)`, `str(i)`, `bool(i)`, `datetime(i)`, `decimal(i)`,
`opt_str(i)`.

## Repository

`Repository<M>` wraps a store with typed methods and is also an axum extractor.

| Method                          | Description                              |
|---------------------------------|------------------------------------------|
| `new(store)`                    | Create from an `Arc<dyn Store>`          |
| `save(&self, &mut model)`       | Insert or update by key                  |
| `save_many(&self, &mut [models])` | Batch insert or update                 |
| `get(&self, &Value)`            | Fetch by primary key                     |
| `all(&self)`                    | All rows, ordered by key                 |
| `filter(&self, Name, &Value)`   | Equality filter                          |
| `ordered(&self, Sort)`          | All rows in a given order                |
| `scan_query(&self, &Query)`     | Arbitrary query into models              |
| `rows(&self, &Query)`           | Arbitrary query into `Row`s              |
| `total_query(&self, &Query)`    | Row count respecting filter and page     |
| `mass(&self, &Query)`           | Aggregation across matching rows         |
| `update(&self, &Model)`         | Replace the row by primary key           |
| `delete(&self, &Value)`         | Delete by primary key                    |

Every repository call creates the table first, so calling `Repository::new`
on a fresh store is enough to bootstrap a schema.

## Store

`Arc<dyn Store>` is the raw persistence interface, built for concurrent access.

| Method                                | Description                                 |
|---------------------------------------|---------------------------------------------|
| `execute(&self, sql, params)`         | Raw SQL with bound parameters               |
| `fetch(&self, sql, params, columns)`  | Raw SQL returning typed `Row`s              |
| `columns(&self, table)`               | Column names and kinds (introspection)      |
| `define(&self, &Schema)`              | Create the table if missing                 |
| `create(&self, &Schema, &[batch])`    | Insert or update a batch, return the keys   |
| `replace(&self, &Schema, &Key, &[pair])` | Update given columns of a row            |
| `upsert(&self, &Schema, &Key, &[pair])`  | Insert or replace a single row           |
| `remove(&self, &Schema, &Key)`        | Delete a row                                |
| `evolve(&self, &Schema, drop)`        | Migrate: create and alter, or drop and recreate |
| `mass(&self, &Schema, &Query)`       | Aggregation                                 |
| `insert(&self, &Schema, &[pair])`    | Insert and return the new key               |
| `deal(&self)`                         | Begin a transaction (see below)             |
| `settle(self: Arc<Self>, commit)`     | Commit or roll the transaction back         |

## Backends

### SQLite

```rust
use rango::store::sqlite;

let store = sqlite::open("store").await?;       // create or reuse the file
let store = sqlite::open_wal("store").await?;   // WAL journal mode
```

Feature `sqlite` is on by default.

### PostgreSQL

```rust
use rango::store::postgres;

let store = postgres::connect("postgres://user@host/db").await?;
```

Feature `postgres` must be enabled in `Cargo.toml`:

```toml
rango = { features = ["postgres"] }
```

Column types per backend:

| Store type | SQLite                        | PostgreSQL                       |
|------------|-------------------------------|----------------------------------|
| `Id`       | `INTEGER PRIMARY KEY AUTOINCREMENT` | `BIGSERIAL PRIMARY KEY`      |
| `Key`      | `TEXT PRIMARY KEY`            | `TEXT PRIMARY KEY`               |
| `Str`      | `TEXT`                        | `TEXT`                           |
| `Int`      | `INTEGER`                     | `BIGINT`                         |
| `Float`    | `REAL`                        | `DOUBLE PRECISION`               |
| `Bool`     | `INTEGER`                     | `BIGINT`                         |
| `Moment`   | `INTEGER` (unix timestamp)    | `BIGINT` (unix timestamp)        |
| `Decimal`  | `TEXT`                        | `TEXT`                           |

Placeholders differ between backends: SQLite uses `?`, PostgreSQL uses
`$1`, `$2`, and so on. Writes use `INSERT OR REPLACE` on SQLite and
`INSERT ... ON CONFLICT ... DO UPDATE` on PostgreSQL. Aggregations are
cast so `Count`, `Sum`, and `Mean` return the same value kinds on both.

## Transactions

Wrap multiple operations in one atomic unit:

```rust
let trade = store.deal().await?;
trade.create(&schema, &batch).await?;
trade.replace(&schema, &key, &updates).await?;
trade.settle(true).await?;    // commit
// trade.settle(false).await?; // roll back
```

`deal()` returns an `Arc<dyn Store>` that works like the normal store.
Call `settle(true)` to commit or `settle(false)` to roll back. A nested
`deal()` inside a transaction returns `StoreError::Unsupported`.