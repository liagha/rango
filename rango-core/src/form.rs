pub use axum::extract::Form;

pub struct FieldError {
    field: String,
    message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

pub struct Errors(Vec<FieldError>);

impl Errors {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.0.push(FieldError::new(field, message));
    }

    pub fn message(&self, field: &str) -> &str {
        self.0
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
            .unwrap_or("")
    }

    pub fn valid(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for Errors {
    fn default() -> Self {
        Self::new()
    }
}

pub trait Valid {
    fn errors(&self) -> Errors;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors() {
        let mut errors = Errors::new();
        assert!(errors.valid());
        assert_eq!(errors.message("name"), "");
        errors.push("name", "required");
        errors.push("name", "too short");
        errors.push("age", "too old");
        assert!(!errors.valid());
        assert_eq!(errors.message("name"), "required");
        assert_eq!(errors.message("age"), "too old");
        assert_eq!(errors.message("missing"), "");
    }
}
