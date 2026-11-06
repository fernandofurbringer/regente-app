use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{message}")]
    Coded { code: &'static str, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl AppError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded { code, message: message.into() }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Coded { code, .. } => code,
            Self::Io(_) => "io",
            Self::Db(_) => "db",
            Self::Json(_) => "json",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        tracing::warn!("{}: {}", self.code(), self);
        let mut st = s.serialize_struct("AppError", 2)?;
        st.serialize_field("code", self.code())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;

pub fn fail<T>(code: &'static str, message: impl Into<String>) -> AppResult<T> {
    Err(AppError::new(code, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serializes_code_and_message() {
        let v = serde_json::to_value(AppError::new("spawn_failed", "CLI ausente")).unwrap();
        assert_eq!(v, serde_json::json!({"code": "spawn_failed", "message": "CLI ausente"}));
    }
    #[test]
    fn io_error_has_io_code() {
        let e: AppError = std::io::Error::other("x").into();
        assert_eq!(e.code(), "io");
    }
}
