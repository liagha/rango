use crate::{Column, ColumnKind, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Table(pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Name(pub &'static str);

impl Table {
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl Name {
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

#[derive(Clone, PartialEq)]
pub enum Type {
    Id,
    Key,
    Str,
    Int,
    Float,
    Bool,
    Moment,
    Decimal,
    Ref,
    Many,
    Opt(Box<Type>),
}

impl Type {
    pub fn optional(self) -> Type {
        Type::Opt(Box::new(self))
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, &Type::Opt(_))
    }

    pub fn flat(&self) -> &Type {
        match self {
            Type::Opt(inner) => inner.flat(),
            kind => kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub options: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check(pub &'static str);

#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    To(Table, Name),
    Via(Table, Name, Name),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    Same(Vec<Name>),
    Hold(Name),
    Said(Check),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    More,
    Less,
    At,
    In,
    Out,
    Like,
    Bare,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub field: Name,
    pub op: Op,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tree {
    Leaf(Filter),
    And(Vec<Tree>),
    Or(Vec<Tree>),
    Cut(Box<Tree>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub field: Name,
    pub order: Order,
}

impl Sort {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub count: usize,
    pub offset: usize,
}

impl Page {
    pub fn all() -> Self {
        Self {
            count: 0,
            offset: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Only {
    All,
    Some(Vec<Name>),
    Lone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mass {
    Count,
    Sum(Name),
    Mean(Name),
    Low(Name),
    High(Name),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub tree: Tree,
    pub sort: Vec<Sort>,
    pub page: Page,
    pub only: Only,
    pub mass: Option<Mass>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Int(i64),
    Text(String),
}

impl Key {
    pub fn parse(raw: &str, schema: &Schema) -> Self {
        let keyed = schema
            .fields
            .iter()
            .any(|field| matches!(field.kind.flat(), Type::Key) && field.name == schema.key());
        if keyed {
            return Key::Text(raw.into());
        }
        match raw.parse::<i64>() {
            Ok(id) => Key::Int(id),
            Err(_) => Key::Text(raw.into()),
        }
    }

    pub fn value(&self) -> Value {
        match self {
            Key::Int(id) => Value::int(*id),
            Key::Text(text) => Value::str(text),
        }
    }
}

#[derive(Clone)]
pub struct Field {
    pub name: Name,
    pub kind: Type,
    pub unique: bool,
    pub index: bool,
    pub pick: Option<Pick>,
    pub default: Option<Value>,
    pub link: Option<Link>,
}

impl Field {
    pub fn new(name: &'static str, kind: Type) -> Self {
        Self {
            name: Name(name),
            kind,
            unique: false,
            index: false,
            pick: None,
            default: None,
            link: None,
        }
    }

    pub fn id() -> Self {
        Self::new("id", Type::Id)
    }

    pub fn key(name: &'static str) -> Self {
        Self::new(name, Type::Key)
    }

    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    pub fn indexed(mut self) -> Self {
        self.index = true;
        self
    }

    pub fn default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    pub fn references(mut self, target: &'static str) -> Self {
        let (table, column) = match target.split_once('.') {
            Some((table, column)) => (table, column),
            None => (target, "id"),
        };
        self.link = Some(Link::To(Table(table), Name(column)));
        self
    }

    pub fn link(mut self, link: Link) -> Self {
        self.link = Some(link);
        self
    }

    pub fn reference(&self) -> Option<(Table, Name)> {
        match self.link {
            Some(Link::To(table, name)) => Some((table, name)),
            Some(Link::Via(..)) => todo!("phase 3"),
            None => None,
        }
    }
}

#[derive(Clone)]
pub struct Schema {
    pub table: Table,
    pub fields: Vec<Field>,
    pub rules: Vec<Rule>,
}

impl Schema {
    pub fn key(&self) -> Name {
        self.fields
            .iter()
            .find(|field| matches!(field.kind.flat(), Type::Id | Type::Key))
            .map(|field| field.name)
            .unwrap_or(Name("id"))
    }

    pub fn kinds(&self) -> Vec<ColumnKind> {
        self.fields
            .iter()
            .map(|field| affinity(&field.kind))
            .collect()
    }

    pub fn ddl(&self) -> String {
        let columns: Vec<String> = self.fields.iter().map(column).collect();
        format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            self.table,
            columns.join(", ")
        )
    }

    pub fn alter(&self, have: &[Column]) -> Vec<String> {
        let moved = self.moved(have);
        let mut out = Vec::new();
        for field in &self.fields {
            if field.kind == Type::Id {
                continue;
            }
            if !have.iter().any(|col| col.name == field.name.as_str())
                && !moved.iter().any(|(_, name)| name == field.name.as_str())
            {
                out.push(format!(
                    "ALTER TABLE \"{}\" ADD COLUMN {}",
                    self.table,
                    column(field)
                ));
            }
        }
        out
    }

    pub fn drop(&self, have: &[Column]) -> Vec<String> {
        let moved = self.moved(have);
        let mut out = Vec::new();
        for col in have {
            if col.name == "id" {
                continue;
            }
            if !self
                .fields
                .iter()
                .any(|field| field.name.as_str() == col.name.as_str())
                && !moved.iter().any(|(name, _)| name == &col.name)
            {
                out.push(format!(
                    "ALTER TABLE \"{}\" DROP COLUMN \"{}\"",
                    self.table, col.name
                ));
            }
        }
        out
    }

    pub fn rename(&self, have: &[Column]) -> Vec<String> {
        self.moved(have)
            .into_iter()
            .map(|(old, name)| {
                format!(
                    "ALTER TABLE \"{}\" RENAME COLUMN \"{old}\" TO \"{name}\"",
                    self.table
                )
            })
            .collect()
    }

    fn moved(&self, have: &[Column]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for kind in [ColumnKind::Integer, ColumnKind::Real, ColumnKind::Text] {
            let gone: Vec<&String> = have
                .iter()
                .filter(|col| {
                    col.name != "id"
                        && col.kind == kind
                        && !self
                            .fields
                            .iter()
                            .any(|field| field.name.as_str() == col.name.as_str())
                })
                .map(|col| &col.name)
                .collect();
            let fresh: Vec<&Field> = self
                .fields
                .iter()
                .filter(|field| {
                    field.kind != Type::Id
                        && affinity(&field.kind) == kind
                        && !have.iter().any(|col| col.name == field.name.as_str())
                })
                .collect();
            if let ([old], [new]) = (gone.as_slice(), fresh.as_slice()) {
                out.push(((*old).clone(), new.name.to_string()));
            }
        }
        out
    }
}

pub(crate) fn affinity(kind: &Type) -> ColumnKind {
    match kind {
        Type::Id | Type::Int | Type::Moment | Type::Bool => ColumnKind::Integer,
        Type::Float => ColumnKind::Real,
        Type::Str | Type::Key | Type::Decimal => ColumnKind::Text,
        Type::Ref => todo!("phase 2"),
        Type::Many => todo!("phase 3"),
        Type::Opt(inner) => affinity(inner),
    }
}

fn literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Str(value) => format!("'{}'", value.replace('\'', "''")),
        Value::Bool(value) => {
            if *value {
                "1".into()
            } else {
                "0".into()
            }
        }
        Value::DateTime(at) => at.timestamp().to_string(),
        Value::Decimal(value) => format!("'{value}'"),
    }
}

fn sql(kind: &Type) -> &'static str {
    match kind {
        Type::Id => "INTEGER PRIMARY KEY AUTOINCREMENT",
        Type::Key => "TEXT PRIMARY KEY",
        Type::Str => "TEXT",
        Type::Int | Type::Moment => "INTEGER",
        Type::Float => "REAL",
        Type::Bool => "INTEGER",
        Type::Decimal => "TEXT",
        Type::Ref => todo!("phase 2"),
        Type::Many => todo!("phase 3"),
        Type::Opt(inner) => sql(inner),
    }
}

fn column(field: &Field) -> String {
    let mut base = sql(&field.kind).to_string();
    if !matches!(field.kind.flat(), Type::Id | Type::Key) {
        if let Some((table, column)) = field.reference() {
            base.push_str(&format!(" REFERENCES \"{table}\"(\"{column}\")"));
        }
        if field.unique {
            base.push_str(" UNIQUE");
        }
        if !field.kind.is_optional() {
            base.push_str(" NOT NULL");
        }
        if let Some(default) = &field.default {
            base.push_str(&format!(" DEFAULT {}", literal(default)));
        }
    }
    format!("\"{}\" {}", field.name, base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, kind: ColumnKind) -> Column {
        Column {
            name: name.to_string(),
            kind,
        }
    }

    fn schema() -> Schema {
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
    fn ddl() {
        assert_eq!(
            schema().ddl(),
            "CREATE TABLE IF NOT EXISTS \"posts\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"title\" TEXT NOT NULL)"
        );
    }

    #[test]
    fn key_ddl() {
        assert_eq!(
            keyed().ddl(),
            "CREATE TABLE IF NOT EXISTS \"products\" (\"sku\" TEXT PRIMARY KEY, \"price\" TEXT NOT NULL)"
        );
    }

    #[test]
    fn adds() {
        let schema = schema();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
        ];
        assert!(schema.alter(&have).is_empty());
        let missing = vec![col("id", ColumnKind::Integer)];
        assert_eq!(schema.alter(&missing).len(), 1);
    }

    #[test]
    fn drops() {
        let schema = schema();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("title", ColumnKind::Text),
            col("junk", ColumnKind::Text),
        ];
        let drop = schema.drop(&have);
        assert_eq!(drop.len(), 1);
        assert!(schema.drop(&have[..2]).is_empty());
    }

    #[test]
    fn renames() {
        let schema = schema();
        let have = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
        ];
        let rename = schema.rename(&have);
        assert_eq!(rename.len(), 1);
        assert!(schema.alter(&have).is_empty());
        assert!(schema.drop(&have).is_empty());
        let mixed = vec![
            col("id", ColumnKind::Integer),
            col("name", ColumnKind::Text),
            col("age", ColumnKind::Integer),
        ];
        assert_eq!(schema.rename(&mixed).len(), 1);
        assert!(schema.alter(&mixed).is_empty());
        assert_eq!(schema.drop(&mixed).len(), 1);
    }

    #[test]
    fn affinities() {
        assert_eq!(
            schema().kinds(),
            vec![ColumnKind::Integer, ColumnKind::Text]
        );
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
