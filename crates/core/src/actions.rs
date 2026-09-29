//! Acciones del sistema que puede pedir la interfaz: abrir enlaces permitidos,
//! abrir carpetas, reiniciar el Explorador y relanzar la app como administrador.

use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::util::{com, shell, win};

const MAX_URI_LEN: usize = 512;
/// Esquemas que se pueden abrir. Todo lo demás se rechaza.
const ALLOWED_SCHEMES: &[&str] = &["ms-settings:", "https:", "windowsdefender:", "ms-windows-store:"];

pub fn validate_uri(uri: &str) -> AppResult<()> {
    let lower = uri.to_ascii_lowercase();
    let allowed = ALLOWED_SCHEMES.iter().any(|scheme| lower.starts_with(scheme));
    if !allowed || uri.len() > MAX_URI_LEN || uri.chars().any(char::is_control) {
        return Err(AppError::invalid("Ese enlace no está permitido."));
    }
    Ok(())
}

/// Abre una URI de la lista blanca con su aplicación predeterminada.
pub fn open_uri(uri: &str) -> AppResult<()> {
    validate_uri(uri)?;
    let uri = uri.to_owned();
    com::sta(move || shell::open(&uri))
}

/// Abre una carpeta local en el Explorador (solo carpetas existentes).
pub fn open_folder(path: &Path) -> AppResult<()> {
    if !path.is_absolute() || !path.is_dir() {
        return Err(AppError::not_found("La carpeta no existe."));
    }
    let target = path.to_string_lossy().into_owned();
    com::sta(move || shell::open(&target))
}

/// ¿La app corre con privilegios de administrador?
pub fn is_admin() -> bool {
    win::is_elevated()
}

/// Relanza el ejecutable actual con UAC. Si el usuario acepta, el llamador debe cerrar la app.
pub fn relaunch_as_admin(args: &str) -> AppResult<()> {
    if win::is_elevated() {
        return Err(AppError::invalid("La app ya se está ejecutando como administrador."));
    }
    let exe = std::env::current_exe()?;
    let args = args.to_owned();
    com::sta(move || shell::run_as_admin(&exe, &args))
}

/// Reinicia el Explorador para aplicar cambios de la barra de tareas y del shell.
pub fn restart_explorer() -> AppResult<()> {
    shell::restart_explorer()
}

#[cfg(test)]
mod tests {
    use super::validate_uri;

    #[test]
    fn uri_allowlist() {
        assert!(validate_uri("ms-settings:display").is_ok());
        assert!(validate_uri("https://www.icloud.com").is_ok());
        assert!(validate_uri("file:///C:/Windows/System32/cmd.exe").is_err());
        assert!(validate_uri("C:\\Windows\\System32\\cmd.exe").is_err());
        assert!(validate_uri("https://ok.com/\nmalicioso").is_err());
    }
}
