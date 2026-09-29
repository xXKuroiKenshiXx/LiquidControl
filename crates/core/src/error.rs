//! Errores del backend. Viajan al frontend serializados como `{ code, message }`
//! y el mensaje ya está redactado para mostrarse al usuario.

use serde::ser::{Serialize, SerializeStruct, Serializer};

/// HRESULT de "acceso denegado" (E_ACCESSDENIED = HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED)).
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;
const ERROR_ACCESS_DENIED: i32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Este cambio requiere permisos de administrador.")]
    NeedsAdmin,
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    /// Error con un código propio que el frontend puede manejar (p. ej. `needs_password`).
    #[error("{1}")]
    Coded(&'static str, String),
    #[error("{0}")]
    Failed(String),
    #[error("Error de E/S: {0}")]
    Io(std::io::Error),
    #[error("Error de Windows: {}", .0.message())]
    Windows(windows::core::Error),
    #[error("Datos inválidos: {0}")]
    Json(#[from] serde_json::Error),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NeedsAdmin => "needs_admin",
            Self::NotFound(_) => "not_found",
            Self::Invalid(_) => "invalid",
            Self::Coded(code, _) => code,
            Self::Failed(_) => "failed",
            Self::Io(_) => "io",
            Self::Windows(_) => "windows",
            Self::Json(_) => "json",
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        if err.raw_os_error() == Some(ERROR_ACCESS_DENIED) || err.kind() == std::io::ErrorKind::PermissionDenied {
            Self::NeedsAdmin
        } else {
            Self::Io(err)
        }
    }
}

impl From<windows::core::Error> for AppError {
    fn from(err: windows::core::Error) -> Self {
        if err.code().0 == E_ACCESSDENIED { Self::NeedsAdmin } else { Self::Windows(err) }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("AppError", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
