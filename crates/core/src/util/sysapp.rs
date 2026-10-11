//! Apps del sistema que Windows ejecuta desde `C:\Windows\SystemApps` (por
//! ejemplo el host del menú Inicio). Se "desactivan" renombrando su carpeta a
//! `<nombre>.old`: Windows ya no la encuentra y esa pieza no arranca. Revertir es
//! devolverle el nombre.
//!
//! El nombre de la carpeta cambia de un Windows a otro (versión, edición,
//! idioma de paquete), así que **no se escribe a mano**: se busca en
//! `SystemApps` la que contiene el ejecutable pedido (p. ej.
//! `StartMenuExperienceHost.exe`), sea la original o la ya renombrada.
//!
//! La carpeta pertenece a TrustedInstaller, así que antes de renombrar se toma
//! su propiedad con `takeown`/`icacls` (herramientas de Windows, por ruta
//! absoluta) y, al volver, se le devuelve a TrustedInstaller. Hace falta
//! administrador; el motor de ajustes ya lo exige antes de llegar acá.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{AppError, AppResult};
use crate::util::{process, win};

const PARKED_SUFFIX: &str = ".old";
const ATTEMPTS: u32 = 10;
const RETRY_EVERY: Duration = Duration::from_millis(300);
/// SID de `BUILTIN\Administrators`: no depende del idioma de Windows.
const ADMINISTRATORS: &str = "*S-1-5-32-544";

/// Dónde está la app en este equipo y si hoy está desactivada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub dir: PathBuf,
    pub disabled: bool,
}

/// Nombre de ejecutable aceptable: sin separadores de ruta ni `..` y con `.exe`.
pub fn valid_exe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && !name.contains("..")
        && name.to_ascii_lowercase().ends_with(".exe")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn base() -> PathBuf {
    win::system_root().join("SystemApps")
}

fn is_parked_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(PARKED_SUFFIX)
}

/// Busca en `SystemApps` la carpeta que trae `exe`. Si hay una en su lugar y
/// otra renombrada (p. ej. Windows la volvió a crear en una actualización), manda la
/// que está en su lugar.
pub fn locate(exe: &str) -> Option<Located> {
    if !valid_exe(exe) {
        return None;
    }
    let mut parked = None;
    for entry in std::fs::read_dir(base()).ok()?.flatten() {
        let dir = entry.path();
        if !dir.join(exe).is_file() {
            continue;
        }
        if is_parked_name(&entry.file_name().to_string_lossy()) {
            parked.get_or_insert(dir);
        } else {
            return Some(Located { dir, disabled: false });
        }
    }
    parked.map(|dir| Located { dir, disabled: true })
}

/// `Some(true)` si la app está desactivada (carpeta renombrada), `Some(false)`
/// si está en su lugar y `None` si este Windows no la trae.
pub fn is_disabled(exe: &str) -> Option<bool> {
    locate(exe).map(|found| found.disabled)
}

fn run_checked(program: &Path, args: &[&str]) -> AppResult<()> {
    let output = process::run_hidden(program, args)?;
    if output.status.success() { Ok(()) } else { Err(AppError::failed(process::summary(&output))) }
}

/// Toma la propiedad de la carpeta y le da control total a los administradores,
/// que es lo mínimo para poder renombrarla.
fn take_control(dir: &Path) -> AppResult<()> {
    let dir = dir.to_string_lossy();
    run_checked(&win::system32("takeown.exe"), &["/F", &dir, "/A"])?;
    run_checked(&win::system32("icacls.exe"), &[&dir, "/grant", &format!("{ADMINISTRATORS}:(F)")])
}

fn rename_with_retries(from: &Path, to: &Path, before_each: impl Fn()) -> AppResult<()> {
    let mut last = None;
    for _ in 0..ATTEMPTS {
        before_each();
        match std::fs::rename(from, to) {
            Ok(()) => {
                log::info!(target: "sistema", "{} → {}", from.display(), to.display());
                return Ok(());
            }
            Err(err) => last = Some(err),
        }
        std::thread::sleep(RETRY_EVERY);
    }
    let detail = last.map_or_else(String::new, |err| format!(": {err}"));
    Err(AppError::failed(format!("No se pudo renombrar {}{detail}", from.display())))
}

/// Desactiva la app que trae `exe`: cierra su proceso (si corre) y renombra su
/// carpeta.
pub fn disable(exe: &str) -> AppResult<()> {
    if !valid_exe(exe) {
        return Err(AppError::failed(format!("Ejecutable inválido: {exe}")));
    }
    let found = locate(exe).ok_or_else(|| AppError::not_found(format!("Este Windows no trae {exe}")))?;
    if found.disabled {
        return Ok(());
    }
    let mut parked_name = found.dir.file_name().map(std::ffi::OsString::from).unwrap_or_default();
    parked_name.push(PARKED_SUFFIX);
    let to = found.dir.with_file_name(parked_name);
    if to.exists() {
        return Err(AppError::failed(format!("Ya existe {}: no se pisa", to.display())));
    }
    take_control(&found.dir)?;
    // Windows relanza el proceso al toque: se lo cierra antes de cada intento.
    let taskkill = win::system32("taskkill.exe");
    rename_with_retries(&found.dir, &to, || {
        let _ = process::run_hidden(&taskkill, &["/F", "/IM", exe]);
    })
}

/// Reactiva la app que trae `exe`: le devuelve su nombre a la carpeta y, si se
/// puede, su propietario original (TrustedInstaller).
pub fn enable(exe: &str) -> AppResult<()> {
    if !valid_exe(exe) {
        return Err(AppError::failed(format!("Ejecutable inválido: {exe}")));
    }
    let found = locate(exe).ok_or_else(|| AppError::not_found(format!("Este Windows no trae {exe}")))?;
    if !found.disabled {
        return Ok(());
    }
    let name = found.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let original = name.get(..name.len() - PARKED_SUFFIX.len()).unwrap_or_default();
    if original.is_empty() {
        return Err(AppError::failed(format!("Nombre de carpeta inesperado: {name}")));
    }
    let to = found.dir.with_file_name(original);
    if to.exists() {
        return Err(AppError::failed(format!("Ya existe {}: no se pisa", to.display())));
    }
    if std::fs::rename(&found.dir, &to).is_err() {
        // Una carpeta renombrada a mano (o por un .bat) sigue siendo de TrustedInstaller.
        take_control(&found.dir)?;
        rename_with_retries(&found.dir, &to, || {})?;
    }
    let owner = to.to_string_lossy();
    let _ = process::run_hidden(&win::system32("icacls.exe"), &[&owner, "/setowner", r"NT SERVICE\TrustedInstaller"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_executable_names_are_accepted() {
        assert!(valid_exe("StartMenuExperienceHost.exe"));
        for bad in ["", "..", "a/b.exe", r"a\b.exe", "a b.exe", "..\\x.exe", "x..y.exe", "C:", "a;b.exe", "notepad"] {
            assert!(!valid_exe(bad), "{bad:?} no debería valer");
        }
        assert_eq!(locate("../cmd.exe"), None);
    }

    #[test]
    fn parked_names_are_recognised() {
        assert!(is_parked_name("Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy.old"));
        assert!(is_parked_name("X.OLD"));
        assert!(!is_parked_name("Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy"));
    }
}
