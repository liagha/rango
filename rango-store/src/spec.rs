use std::sync::Arc;

use crate::{BoxFuture, Gather, Reader, Show, Storable, Store, StoreError, Value, Widget, Writer};

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
    pub fn of(value: &Value) -> Result<Self, StoreError> {
        match value {
            Value::Int(id) => Ok(Key::Int(*id)),
            Value::Str(text) => Ok(Key::Text(text.clone())),
            _ => Err(StoreError::Value("bad key".into())),
        }
    }

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
    pub dtype: &'static str,
    pub id: bool,
    pub keyed: bool,
    pub many: bool,
    pub optional: bool,
    pub unique: bool,
    pub index: bool,
    pub pick: Option<Pick>,
    pub default: Option<Arc<dyn Fn(&mut dyn Writer) + Send + Sync + 'static>>,
    pub link: Option<Link>,
    pub widget: fn() -> Widget,
    pub parse: fn(&str, &mut dyn Writer) -> Result<(), StoreError>,
    pub text: fn(&mut dyn Reader) -> String,
}

impl Field {
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
            pick: None,
            default: None,
            link: None,
            widget: widget_of::<T>,
            parse: parse_of::<T>,
            text: text_of::<T>,
        }
    }

    pub fn id() -> Self {
        let field = Self::cell::<i64>("id");
        Self {
            id: true,
            keyed: true,
            ..field
        }
    }

    pub fn key<T: Storable + Show>(name: &'static str) -> Self {
        let field = Self::cell::<T>(name);
        Self {
            keyed: true,
            ..field
        }
    }

    pub fn str(name: &'static str) -> Self {
        Self::cell::<String>(name)
    }

    pub fn check(name: &'static str) -> Self {
        Self::cell::<bool>(name)
    }

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
            pick: None,
            default: None,
            link: None,
            widget: nop_widget,
            parse: nop_parse,
            text: nop_text,
        }
    }

    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    pub fn indexed(mut self) -> Self {
        self.index = true;
        self
    }

    pub fn default_value(mut self, value: impl Storable) -> Self {
        self.default = Some(Arc::new(move |w| value.put(w)));
        self
    }

    pub fn default(mut self, make: impl Fn(&mut dyn Writer) + Send + Sync + 'static) -> Self {
        self.default = Some(Arc::new(make));
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

    pub fn load(&self) -> Widget {
        (self.widget)()
    }

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

fn nop_widget() -> Widget {
    Widget::Text
}

fn nop_parse(_raw: &str, _w: &mut dyn Writer) -> Result<(), StoreError> {
    Ok(())
}

fn nop_text(_r: &mut dyn Reader) -> String {
    String::new()
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
            .find(|field| field.id || field.keyed)
            .map(|field| field.name)
            .unwrap_or(Name("id"))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Action {
    pub name: Name,
    pub title: &'static str,
    pub run: Run,
    pub logged: bool,
    pub row: bool,
}

pub type Run =
    fn(Arc<dyn Store>, Schema, Vec<Key>) -> BoxFuture<'static, Result<String, StoreError>>;

impl Action {
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