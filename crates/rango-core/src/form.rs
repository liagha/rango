pub use axum::extract::Form;

pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

pub struct Errors(pub Vec<FieldError>);

impl Errors {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.0.push(FieldError::new(field, message));
    }

    pub fn get(&self, field: &str) -> Option<&FieldError> {
        self.0.iter().find(|error| error.field == field)
    }

    pub fn message(&self, field: &str) -> &str {
        self.get(field)
            .map(|error| error.message.as_str())
            .unwrap_or("")
    }

    pub fn valid(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<FieldError>> for Errors {
    fn from(fields: Vec<FieldError>) -> Self {
        Self(fields)
    }
}

impl Default for Errors {
    fn default() -> Self {
        Self::new()
    }
}

impl IntoIterator for Errors {
    type Item = FieldError;
    type IntoIter = std::vec::IntoIter<FieldError>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

pub trait Valid {
    fn errors(&self) -> Errors;

    fn valid(&self) -> bool {
        self.errors().valid()
    }

    fn clean(self) -> Result<Self, Errors>
    where
        Self: Sized,
    {
        let errors = self.errors();
        if errors.valid() {
            Ok(self)
        } else {
            Err(errors)
        }
    }
}
