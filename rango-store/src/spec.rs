use std::sync::Arc;

use crate::{BoxFuture, Column, ColumnKind, Gather, Reader, Show, Storable, Store, StoreError, Value, Widget, Writer};

/// A table name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Table(pub &'static str);

/// A column name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Name(pub &'static str);

impl Table {
    /// The underlying `&'static str`.
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl Name {
    /// The underlying `&'static str`.
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// A fixed (value, label) option for a field's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The stored value.
    pub value: Name,
    /// The label shown to users.
    pub label: &'static str,
}

impl Choice {
    /// A choice from its stored value and label.
    pub const fn of(value: &'static str, label: &'static str) -> Self {
        Self {
            value: Name(value),
            label,
        }
    }
}

/// Reference between tables.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    /// Direct reference to a column of another table.
    To(Table, Name),
    /// Many-to-many through a join table: table, our column, their column.
    Via(Table, Name, Name),
}

/// Table-level constraint appended to the create statement.
#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    /// Unique across the given columns.
    Unique(Vec<Name>),
    /// Raw check expression, e.g. `"price >= 0"`.
    Check(&'static str),
}

/// Comparison operator for a [`Filter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Strictly greater.
    More,
    /// Strictly less.
    Less,
    /// Within the given calendar day.
    At,
    /// Case-insensitive SQL `LIKE` match.
    Like,
    /// `IS NULL` on the field.
    Bare,
}

/// A leaf condition on one field.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    /// Field to compare.
    pub field: Name,
    /// Comparison operator.
    pub op: Op,
    /// Comparison value.
    pub value: Value,
}

/// Nested query condition.
#[derive(Clone, Debug, PartialEq)]
pub enum Tree {
    /// A single leaf filter.
    Leaf(Filter),
    /// All branches must hold.
    And(Vec<Tree>),
    /// At least one branch must hold.
    Or(Vec<Tree>),
    /// Negation of the wrapped branch.
    Cut(Box<Tree>),
}

/// Sort direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}

/// A sort on one field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    /// Field to sort by.
    pub field: Name,
    /// Sort direction.
    pub order: Order,
}

impl Sort {
    /// Parses `-name`/`name` into a [`Sort`]; unknown fields fall back to the schema key.
    pub fn parse(raw: &str, schema: &Schema) -> Self {
        let (name, order) = match raw.strip_prefix('-') {
            Some(name) => (name, Order::Desc),
            None => (raw, Order::Asc),
        };
        let known = schema
            .fields
            .iter()
            .any(|field| field.name.as_str() == name);
        if known {
            for field in &schema.fields {
                if field.name.as_str() == name {
                    return Sort {
                        field: field.name,
                        order,
                    };
                }
            }
        }
        Sort {
            field: schema.key(),
            order: Order::Asc,
        }
    }
}

/// Paging window; `count` 0 means no limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// Maximum rows, 0 for all.
    pub count: usize,
    /// Rows to skip.
    pub offset: usize,
}

impl Page {
    /// The full page: no limit, no offset.
    pub fn all() -> Self {
        Self {
            count: 0,
            offset: 0,
        }
    }
}

/// Which columns a query returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Only {
    /// Every physical column.
    All,
    /// Only the named columns.
    Some(Vec<Name>),
    /// First matching row, all columns.
    Lone,
}

/// Aggregate over matching rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mass {
    /// Row count.
    Count,
    /// Sum of a column.
    Sum(Name),
    /// Average of a column.
    Mean(Name),
    /// Minimum of a column.
    Low(Name),
    /// Maximum of a column.
    High(Name),
}

/// A full read: condition, sort, page, columns, optional aggregate.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// Root condition.
    pub tree: Tree,
    /// Ordering.
    pub sort: Vec<Sort>,
    /// Paging window.
    pub page: Page,
    /// Columns to return.
    pub only: Only,
    /// Optional aggregate.
    pub mass: Option<Mass>,
}

/// A row key: autoincrement id or text key.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    /// Autoincrement `id` key.
    Int(i64),
    /// Custom text key.
    Text(String),
}

impl Key {
    /// Converts a [`Value`] into a key, or [`StoreError::Value`].
    pub fn of(value: &Value) -> Result<Self, StoreError> {
        match value {
            Value::Int(id) => Ok(Key::Int(*id)),
            Value::Str(text) => Ok(Key::Text(text.clone())),
            _ => Err(StoreError::Value("bad key".into())),
        }
    }

    /// Parses raw text into a key: text when the schema is keyed, else int-or-text.
    pub fn parse(raw: &str, schema: &Schema) -> Self {
        let keyed = schema
            .fields
            .iter()
            .any(|field| field.keyed && !field.id && field.name == schema.key());
        if keyed {
            return Key::Text(raw.into());
        }
        match raw.parse::<i64>() {
            Ok(id) => Key::Int(id),
            Err(_) => Key::Text(raw.into()),
        }
    }

    /// This key as a [`Value`].
    pub fn value(&self) -> Value {
        match self {
            Key::Int(id) => Value::int(*id),
            Key::Text(text) => Value::str(text),
        }
    }
}

/// Spec for one column of a schema.
#[derive(Clone)]
pub struct Field {
    /// Column name.
    pub name: Name,
    /// SQL type name (`INTEGER`, `REAL`, or `TEXT`).
    pub dtype: &'static str,
    /// Autoincrement primary key column.
    pub id: bool,
    /// Primary key with a caller-supplied value.
    pub keyed: bool,
    /// Virtual many-to-many column, not stored inline.
    pub many: bool,
    /// Nullable column.
    pub optional: bool,
    /// Unique constraint.
    pub unique: bool,
    /// Indexed column.
    pub index: bool,
    /// Fixed (value, label) choices rendering as a select; empty means a plain input.
    pub choices: &'static [Choice],
    /// Default value factory writing into a [`Writer`].
    pub default: Option<DefaultFn>,
    /// Reference to another table, if any.
    pub link: Option<Link>,
    /// Delete policy for the referenced row.
    pub on_delete: Policy,
    /// Widget for this field.
    pub widget: fn() -> Widget,
    /// Parses raw text into the field's value.
    pub parse: fn(&str, &mut dyn Writer) -> Result<(), StoreError>,
    /// Renders the field's value as text.
    pub text: fn(&mut dyn Reader) -> String,
}

/// Default value factory writing into a [`Writer`].
pub type DefaultFn = Arc<dyn Fn(&mut dyn Writer) + Send + Sync + 'static>;

impl Field {
    /// A plain column storing any [`Storable`] + [`Show`] type.
    pub fn cell<T: Storable + Show>(name: &'static str) -> Self {
        Self {
            name: Name(name),
            dtype: T::dtype(),
            id: false,
            keyed: false,
            many: false,
            optional: false,
            unique: false,
            index: false,
            choices: &[],
            default: None,
            link: None,
            on_delete: Policy::Cascade,
            widget: widget_of::<T>,
            parse: parse_of::<T>,
            text: text_of::<T>,
        }
    }

    /// An autoincrement `id` primary key column.
    pub fn id() -> Self {
        let field = Self::cell::<i64>("id");
        Self {
            id: true,
            keyed: true,
            ..field
        }
    }

    /// A caller-supplied primary key column.
    pub fn key<T: Storable + Show>(name: &'static str) -> Self {
        let field = Self::cell::<T>(name);
        Self {
            keyed: true,
            ..field
        }
    }

    /// A string column.
    pub fn str(name: &'static str) -> Self {
        Self::cell::<String>(name)
    }

    /// A boolean column.
    pub fn check(name: &'static str) -> Self {
        Self::cell::<bool>(name)
    }

    /// A virtual many-to-many column.
    pub fn many(name: &'static str) -> Self {
        Self {
            name: Name(name),
            dtype: "TEXT",
            id: false,
            keyed: false,
            many: true,
            optional: false,
            unique: false,
            index: false,
            choices: &[],
            default: None,
            link: None,
            on_delete: Policy::Cascade,
            widget: || Widget::Text,
            parse: |_raw, _w| Ok(()),
            text: |_r| String::new(),
        }
    }

    /// Marks the column nullable.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// Fixed (value, label) choices rendering as a select.
    pub fn choices(mut self, options: &'static [Choice]) -> Self {
        self.choices = options;
        self.widget = || Widget::Choice;
        self
    }

    /// Adds a unique constraint.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Marks the column indexed.
    pub fn indexed(mut self) -> Self {
        self.index = true;
        self
    }

    /// Sets a static default [`Storable`] value.
    pub fn default_value(mut self, value: impl Storable) -> Self {
        self.default = Some(Arc::new(move |w| value.put(w)));
        self
    }

    /// Sets a default value factory.
    pub fn default(mut self, make: impl Fn(&mut dyn Writer) + Send + Sync + 'static) -> Self {
        self.default = Some(Arc::new(make));
        self
    }

    /// Links to another table's column (`"table.column"` or just `"table"`).
    pub fn references(mut self, target: &'static str) -> Self {
        let (table, column) = match target.split_once('.') {
            Some((table, column)) => (table, column),
            None => (target, "id"),
        };
        self.link = Some(Link::To(Table(table), Name(column)));
        self
    }

    /// Sets a [`Link`] to another table.
    pub fn link(mut self, link: Link) -> Self {
        self.link = Some(link);
        self
    }

    /// Sets the delete policy for a referenced row.
    pub fn on_delete(mut self, policy: Policy) -> Self {
        self.on_delete = policy;
        self
    }

    /// The referenced (table, column) for a direct link, if any.
    pub fn reference(&self) -> Option<(Table, Name)> {
        match self.link {
            Some(Link::To(table, name)) => Some((table, name)),
            _ => None,
        }
    }

    /// The field's [`Widget`].
    pub fn load(&self) -> Widget {
        (self.widget)()
    }

    /// The field's default as a [`Value`], or [`Value::Null`].
    pub fn initial(&self) -> Value {
        match &self.default {
            Some(make) => {
                let mut gather = Gather::new();
                make(&mut gather);
                gather.value()
            }
            None => Value::Null,
        }
    }
}

fn widget_of<T: Show>() -> Widget {
    T::widget()
}

fn parse_of<T: Storable + Show>(raw: &str, w: &mut dyn Writer) -> Result<(), StoreError> {
    let value = T::parse(raw)?;
    T::put(&value, w);
    Ok(())
}

fn text_of<T: Storable + Show>(r: &mut dyn Reader) -> String {
    match T::take(r) {
        Ok(value) => T::text(&value),
        Err(_) => String::new(),
    }
}

/// A table definition: name, fields, and rules.
#[derive(Clone)]
pub struct Schema {
    /// Table name.
    pub table: Table,
    /// Column specs.
    pub fields: Vec<Field>,
    /// Constraints.
    pub rules: Vec<Rule>,
}

impl Schema {
    /// The primary key column name.
    pub fn key(&self) -> Name {
        self.fields
            .iter()
            .find(|field| field.id || field.keyed)
            .map(|field| field.name)
            .unwrap_or(Name("id"))
    }

    /// Storage kinds of the physical columns, in field order.
    pub fn kinds(&self) -> Vec<ColumnKind> {
        let mut out = Vec::new();
        for field in &self.fields {
            if !field.many {
                out.push(ColumnKind::of(field.dtype));
            }
        }
        out
    }

    /// Storage kind of a physical column, or a [`StoreError`] for a virtual or missing one.
    pub fn kind(&self, name: Name) -> Result<ColumnKind, StoreError> {
        let field = self
            .fields
            .iter()
            .find(|field| field.name == name)
            .ok_or_else(|| StoreError::Value(format!("unknown column {name}")))?;
        if field.many {
            return Err(StoreError::Value(format!("virtual column {name}")));
        }
        Ok(ColumnKind::of(field.dtype))
    }

    /// `ORDER BY` clause for the given sorts, defaulting to the key.
    pub fn order(&self, sorts: &[Sort]) -> String {
        if sorts.is_empty() {
            return format!("\"{}\"", self.key());
        }
        let mut out = Vec::new();
        for sort in sorts {
            let field = match sort.order {
                Order::Asc => format!("\"{}\"", sort.field),
                Order::Desc => format!("\"{}\" DESC", sort.field),
            };
            out.push(field);
        }
        out.join(", ")
    }

    /// Same-kind column pairs where a stored column was renamed to a schema field.
    pub fn moved(&self, have: &[Column]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for kind in [ColumnKind::Integer, ColumnKind::Real, ColumnKind::Text] {
            let gone = have
                .iter()
                .filter(|col| col.kind == kind && col.name != "id")
                .filter(|col| !self.fields.iter().any(|field| field.name.as_str() == col.name))
                .collect::<Vec<_>>();
            let fresh = self
                .fields
                .iter()
                .filter(|field| !field.id && !field.many && ColumnKind::of(field.dtype) == kind)
                .filter(|field| !have.iter().any(|col| col.name == field.name.as_str()))
                .collect::<Vec<_>>();
            if let ([old], [new]) = (gone.as_slice(), fresh.as_slice()) {
                out.push((old.name.clone(), new.name.as_str().to_string()));
            }
        }
        out
    }

    /// `DROP COLUMN` statements for stored columns that are neither the key, a schema field,
    /// a renamed old name, nor referenced by another table.
    pub fn drop(&self, have: &[Column], refs: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for col in have {
            if col.name == "id" || refs.contains(&col.name) {
                continue;
            }
            let known = self.fields.iter().any(|field| field.name.as_str() == col.name);
            let moved = self.moved(have);
            let renamed = moved.iter().any(|(old, _)| old == &col.name);
            if !known && !renamed {
                out.push(format!(
                    "ALTER TABLE \"{}\" DROP COLUMN \"{}\"",
                    self.table, col.name
                ));
            }
        }
        out
    }

    /// `RENAME COLUMN` statements for moved columns that are not referenced by another table.
    pub fn rename(&self, have: &[Column], refs: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for (old, new) in self.moved(have) {
            if !refs.contains(&old) {
                out.push(format!(
                    "ALTER TABLE \"{}\" RENAME COLUMN \"{old}\" TO \"{new}\"",
                    self.table
                ));
            }
        }
        out
    }
}

/// A bulk row action with a name, title, and handler.
#[derive(Clone, Copy, Debug)]
pub struct Action {
    /// Machine name.
    pub name: Name,
    /// Display title.
    pub title: &'static str,
    /// Handler producing a result message.
    pub run: Run,
    /// Whether the action is logged.
    pub logged: bool,
    /// Whether the action applies to selected rows.
    pub row: bool,
}

/// Action handler: store, schema, and target keys to a result message.
pub type Run =
    fn(Arc<dyn Store>, Schema, Vec<Key>) -> BoxFuture<'static, Result<String, StoreError>>;

impl Action {
    /// The built-in delete action.
    pub fn wipe() -> Self {
        Self {
            name: Name("wipe"),
            title: "Delete",
            run: wipe,
            logged: true,
            row: true,
        }
    }
}

/// How a foreign key behaves when the referenced row is deleted.
#[derive(Clone, Copy, Debug)]
pub enum Policy {
    /// Delete child rows along with the referenced parent.
    Cascade,
    /// Refuse to delete a referenced parent.
    Protect,
    /// Null the reference when the parent is deleted.
    Set,
    /// Leave the reference; the database rejects parent deletes.
    Nothing,
}

fn wipe(
    store: Arc<dyn Store>,
    schema: Schema,
    keys: Vec<Key>,
) -> BoxFuture<'static, Result<String, StoreError>> {
    Box::pin(async move {
        for key in &keys {
            store.remove(&schema, key).await?;
        }
        if keys.len() == 1 {
            Ok("Deleted 1 row.".into())
        } else {
            Ok(format!("Deleted {} rows.", keys.len()))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn schema() -> Schema {
        Schema {
            table: Table("posts"),
            fields: vec![Field::id(), Field::str("title")],
            rules: Vec::new(),
        }
    }

    fn keyed() -> Schema {
        Schema {
            table: Table("products"),
            fields: vec![Field::key::<String>("sku"), Field::cell::<Decimal>("price")],
            rules: Vec::new(),
        }
    }

    #[test]
    fn keys() {
        assert_eq!(schema().key(), Name("id"));
        assert_eq!(keyed().key(), Name("sku"));
        assert_eq!(Key::parse("7", &schema()).value(), Value::int(7));
        assert_eq!(Key::parse("7", &keyed()).value(), Value::str("7"));
        assert_eq!(Key::parse("x", &schema()).value(), Value::str("x"));
    }

    #[test]
    fn key_of() {
        assert_eq!(Key::of(&Value::int(3)).unwrap(), Key::Int(3));
        assert_eq!(Key::of(&Value::str("x")).unwrap(), Key::Text("x".into()));
        assert!(Key::of(&Value::Null).is_err());
    }

    #[test]
    fn sorts() {
        let schema = schema();
        assert_eq!(
            Sort::parse("title", &schema),
            Sort {
                field: Name("title"),
                order: Order::Asc,
            }
        );
        assert_eq!(
            Sort::parse("-title", &schema),
            Sort {
                field: Name("title"),
                order: Order::Desc,
            }
        );
        assert_eq!(
            Sort::parse("junk", &schema),
            Sort {
                field: Name("id"),
                order: Order::Asc,
            }
        );
    }
}
