use crate::Value;

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
    Many,
    Opt(Box<Type>),
}

pub fn many(kind: &Type) -> bool {
    matches!(kind.flat(), Type::Many)
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
    pub fn of(value: &Value) -> Result<Self, crate::StoreError> {
        match value {
            Value::Int(id) => Ok(Key::Int(*id)),
            Value::Str(text) => Ok(Key::Text(text.clone())),
            _ => Err(crate::StoreError::Value("bad key".into())),
        }
    }

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
            _ => None,
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
