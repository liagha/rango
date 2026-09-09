use rango_store::{Row, StoreError, Value};

#[derive(Clone, PartialEq)]
pub enum Kind {
    Id,
    Str,
    Int,
    Float,
    Bool,
    DateTime,
    Optional(Box<Kind>),
}

impl Kind {
    pub fn optional(self) -> Kind {
        Kind::Optional(Box::new(self))
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, &Kind::Optional(_))
    }

    fn without(self) -> Kind {
        match self {
            Kind::Optional(inner) => *inner,
            kind => kind,
        }
    }
}

#[derive(Clone)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub unique: bool,
    pub default: Option<Value>,
}

impl Field {
    pub fn new(name: &'static str, kind: Kind) -> Self {
        Self {
            name,
            kind,
            unique: false,
            default: None,
        }
    }

    pub fn id() -> Self {
        Self::new("id", Kind::Id)
    }

    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    pub fn default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }
}

pub trait Model: Clone + Send + Sync + 'static {
    fn table() -> &'static str;
    fn fields() -> Vec<Field>;
    fn row(&self) -> Vec<Value>;
    fn from_row(row: &Row) -> Result<Self, StoreError>;
    fn set_id(&mut self, id: i64);
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn literal(value: &Value) -> String {
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
    }
}

fn column(field: &Field) -> String {
    let optional = field.kind.is_optional();
    let mut base: String = match field.kind.clone().without() {
        Kind::Id => "INTEGER PRIMARY KEY AUTOINCREMENT".into(),
        Kind::Str => "TEXT".into(),
        Kind::Int | Kind::DateTime => "INTEGER".into(),
        Kind::Float => "REAL".into(),
        Kind::Bool => "INTEGER".into(),
        Kind::Optional(_) => unreachable!(),
    };
    if field.kind != Kind::Id {
        if field.unique {
            base.push_str(" UNIQUE");
        }
        if !optional {
            base.push_str(" NOT NULL");
        }
        if let Some(default) = &field.default {
            base.push_str(&format!(" DEFAULT {}", literal(default)));
        }
    }
    format!("{} {}", field.name, base)
}

pub fn create_table<T: Model>() -> String {
    let columns: Vec<String> = T::fields().iter().map(column).collect();
    format!(
        "CREATE TABLE IF NOT EXISTS {} ({})",
        T::table(),
        columns.join(", ")
    )
}
