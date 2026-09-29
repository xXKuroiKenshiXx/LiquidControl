//! Íconos de apps para la interfaz:
//! - apps instaladas: el ícono de su ejecutable, que entrega el propio Windows;
//! - catálogo para instalar: el ícono público del sitio oficial de cada app,
//!   descargado una sola vez y guardado en caché para que después funcione sin conexión.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::error::{AppError, AppResult};
use crate::{net, paths};

const MAX_ICON_BYTES: usize = 512 * 1024;
/// Si un sitio no ofrece ícono, no se vuelve a intentar hasta dentro de una semana.
const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// Rutas estándar donde los sitios publican su ícono (primero el de mejor calidad).
const CANDIDATES: [&str; 2] = ["apple-touch-icon.png", "favicon.ico"];
const EXTENSIONS: [&str; 2] = ["png", "ico"];

fn cache_dir() -> AppResult<PathBuf> {
    Ok(paths::data_dir()?.join("cache").join("icons"))
}

/// Nombre de archivo seguro a partir del id del paquete.
fn file_stem(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

/// Ícono ya descargado (no usa la red).
pub fn cached(id: &str) -> Option<PathBuf> {
    let dir = cache_dir().ok()?;
    let stem = file_stem(id);
    EXTENSIONS.iter().map(|ext| dir.join(format!("{stem}.{ext}"))).find(|path| path.is_file())
}

fn recently_missing(marker: &PathBuf) -> bool {
    fs::metadata(marker)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .is_some_and(|age| age < RETRY_AFTER)
}

/// Devuelve el ícono del catálogo: desde la caché o, si no está, desde el sitio oficial.
/// Sin conexión devuelve un error y la interfaz muestra el ícono genérico.
pub fn fetch(id: &str, site: &str) -> AppResult<PathBuf> {
    if let Some(path) = cached(id) {
        return Ok(path);
    }
    let dir = cache_dir()?;
    let stem = file_stem(id);
    let marker = dir.join(format!("{stem}.none"));
    if recently_missing(&marker) {
        return Err(AppError::not_found("El sitio no ofrece un ícono."));
    }
    let mut reached_server = false;
    for candidate in CANDIDATES {
        let bytes = match net::get(&format!("https://{site}/{candidate}"), MAX_ICON_BYTES) {
            Ok(bytes) => bytes,
            Err(err) => {
                reached_server |= err.code() == "http_status";
                continue;
            }
        };
        reached_server = true;
        let ext = match bytes.get(..4) {
            Some([0x89, b'P', b'N', b'G']) => "png",
            Some([0, 0, 1, 0]) => "ico",
            _ => continue,
        };
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{stem}.{ext}"));
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &path)?;
        log::info!(target: "red", "Se descargó el ícono de {id} desde {site}");
        return Ok(path);
    }
    // Solo se recuerda la falta de ícono si el sitio respondió: sin conexión se reintenta después.
    if reached_server {
        fs::create_dir_all(&dir)?;
        let _ = fs::write(&marker, b"");
    }
    Err(AppError::not_found("No se pudo obtener el ícono."))
}

#[cfg(test)]
mod tests {
    use super::file_stem;

    #[test]
    fn safe_file_names() {
        assert_eq!(file_stem("Flow-Launcher.Flow-Launcher"), "Flow-Launcher.Flow-Launcher");
        assert_eq!(file_stem("Notepad++.Notepad++"), "Notepad__.Notepad__");
        assert_eq!(file_stem("..\\..\\evil"), ".._.._evil");
        assert!(!file_stem("a/b:c").contains(['/', '\\', ':']));
    }
}
