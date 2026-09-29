//! Carpeta de datos y archivos JSON de la app (preferencias, diario de ajustes).
//!
//! Modo portable: si junto al ejecutable hay un archivo `portable.txt`, todo se
//! guarda en `data\` al lado del `.exe` (ideal para un pendrive). Si no, en
//! `%APPDATA%\LiquidControl`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{AppError, AppResult};

const APP_FOLDER: &str = "LiquidControl";
const PORTABLE_MARKER: &str = "portable.txt";

/// ¿La app corre en modo portable?
pub fn is_portable() -> bool {
    exe_dir().is_some_and(|dir| dir.join(PORTABLE_MARKER).exists())
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Carpeta donde se guardan los datos de la app.
pub fn data_dir() -> AppResult<PathBuf> {
    if let Some(dir) = exe_dir().filter(|dir| dir.join(PORTABLE_MARKER).exists()) {
        return Ok(dir.join("data"));
    }
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join(APP_FOLDER))
        .ok_or_else(|| AppError::failed("No se encontró la carpeta %APPDATA%."))
}

/// Quita la marca BOM que agregan algunos editores (JSON no la admite).
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Lee un JSON; si no existe o está dañado devuelve un objeto vacío.
pub fn read_json(path: &Path) -> AppResult<Value> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str(strip_bom(&text)).unwrap_or_else(|_| json!({}))),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(json!({})),
        Err(err) => Err(err.into()),
    }
}

/// Escritura atómica: se escribe un temporal y se renombra (nunca queda un archivo a medias).
pub fn write_atomic(path: &Path, contents: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_bom;

    #[test]
    fn tolerates_bom() {
        assert_eq!(strip_bom("\u{feff}{\"a\":1}"), "{\"a\":1}");
        assert_eq!(strip_bom("{}"), "{}");
    }
}
