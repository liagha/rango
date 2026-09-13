//! Form extraction and validation.

pub use axum::extract::Form;

/// Validation message for a single field.
pub struct FieldError {
    field: String,
    message: String,
}

impl FieldError {
    /// New error for the given field with the given message.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

/// Validation errors keyed by field name.
pub struct Errors(Vec<FieldError>);

impl Errors {
    /// Empty error set.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Add an error for the given field.
    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.0.push(FieldError::new(field, message));
    }

    /// First message for the given field, or empty if none.
    pub fn message(&self, field: &str) -> &str {
        self.0
            .iter()
            .find(|error| error.field == field)
            .map(|error| error.message.as_str())
            .unwrap_or("")
    }

    /// True when the form has no errors.
    pub fn valid(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for Errors {
    fn default() -> Self {
        Self::new()
    }
}

/// Types that report their validation errors.
pub trait Valid {
    /// Validation errors for this value.
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
