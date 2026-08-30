use std::fmt;

/// Application error type returned from Tauri commands.
#[derive(Debug)]
pub enum AppError {
    Database(rusqlite::Error),
    NotFound(String),
    InvalidOperation(String),
    /// A `git` subprocess failed or could not be launched.
    Git(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Database(e) => write!(f, "Database error: {e}"),
            AppError::NotFound(msg) => write!(f, "Not found: {msg}"),
            AppError::InvalidOperation(msg) => write!(f, "Invalid operation: {msg}"),
            AppError::Git(msg) => write!(f, "Git error: {msg}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Database(e)
    }
}

// Tauri commands require the error type to implement Serialize so it can be
// forwarded to the frontend as a JSON string.
impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
