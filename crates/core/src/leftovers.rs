//! Desinstalación profunda: después del desinstalador de una app, busca lo que
//! suele quedar (carpetas de programa y de datos, accesos directos y claves del
//! registro) para que el usuario elija qué borrar.
//!
//! Reglas de seguridad:
//! - solo se proponen rutas dentro de carpetas conocidas (Archivos de programa,
//!   AppData, ProgramData, menú Inicio, Escritorio, Temp) y nunca esas carpetas
//!   ni nada de Windows;
//! - solo se puede borrar algo que salió de la última búsqueda (por número);
//! - los archivos van a la Papelera y cada clave del registro se exporta a un
//!   `.reg` en la carpeta de datos antes de borrarse.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use windows::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW,
};
use windows::core::PCWSTR;

use crate::error::{AppError, AppResult};
use crate::paths;
use crate::util::win::system32;
use crate::util::{com, process, reg};

/// Carpetas muy grandes se cuentan hasta este número de archivos.
const MAX_COUNTED_FILES: usize = 50_000;
/// Nombres demasiado genéricos para buscar por ellos.
const GENERIC: [&str; 22] = [
    "microsoft",
    "windows",
    "common",
    "commonfiles",
    "google",
    "intel",
    "nvidia",
    "amd",
    "apple",
    "packages",
    "temp",
    "programs",
    "system",
    "update",
    "updater",
    "app",
    "apps",
    "application",
    "software",
    "setup",
    "installer",
    "java",
];
const COMPANY_SUFFIXES: [&str; 13] =
    ["inc", "llc", "ltd", "limited", "corporation", "corp", "gmbh", "co", "company", "sa", "srl", "ag", "bv"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LeftoverKind {
    Folder,
    File,
    RegistryKey,
}

#[derive(Debug, Clone, Serialize)]
pub struct Leftover {
    pub id: usize,
    pub kind: LeftoverKind,
    pub path: String,
    /// Bytes (carpetas y archivos).
    pub size: u64,
    /// Por qué se propone ("Carpeta de instalación", "Datos en AppData"…).
    pub reason: String,
    /// Coincidencia segura: se propone marcada.
    pub recommended: bool,
}

/// Datos de la app, tomados antes de desinstalarla.
#[derive(Debug, Clone)]
pub struct Target {
    pub name: String,
    pub publisher: String,
    pub install_location: Option<PathBuf>,
    pub registry_key: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub removed: usize,
    /// (ruta, motivo) de lo que no se pudo borrar.
    pub failed: Vec<(String, String)>,
    pub needs_admin: bool,
}

static LAST_SCAN: Mutex<Vec<Leftover>> = Mutex::new(Vec::new());

/// Minúsculas y solo letras y números: "7-Zip" → "7zip".
fn normalize(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn is_version(word: &str) -> bool {
    let word = word.trim_start_matches(['v', 'V']);
    !word.is_empty()
        && word.chars().all(|c| c.is_ascii_digit() || c == '.')
        && word.starts_with(|c: char| c.is_ascii_digit())
}

fn is_arch(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "x64" | "x86" | "64-bit" | "32-bit" | "amd64" | "arm64" | "(x64)" | "(x86)"
    )
}

/// Nombre sin versión, arquitectura ni paréntesis: "7-Zip 23.01 (x64)" → "7zip".
fn core_name(name: &str) -> String {
    let mut plain = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let words: Vec<&str> = plain.split_whitespace().filter(|word| !is_version(word) && !is_arch(word)).collect();
    normalize(&words.join(" "))
}

fn company_name(publisher: &str) -> String {
    let words: Vec<&str> = publisher
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|word| !word.is_empty() && !COMPANY_SUFFIXES.contains(&normalize(word).as_str()))
        .collect();
    normalize(&words.join(" "))
}

fn usable(name: &str) -> bool {
    name.chars().count() >= 4 && !GENERIC.contains(&name)
}

/// Nombres por los que se busca: el de la app y, si empieza con el de la empresa, sin ella.
fn candidate_names(target: &Target) -> Vec<String> {
    let core = core_name(&target.name);
    let company = company_name(&target.publisher);
    let mut names = vec![core.clone()];
    if !company.is_empty() && core.starts_with(&company) && core.len() > company.len() {
        names.push(core[company.len()..].to_owned());
    }
    names.retain(|name| usable(name));
    names.dedup();
    names
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|path| path.is_dir())
}

/// Carpetas donde se busca, con el motivo que se muestra.
fn search_roots() -> Vec<(PathBuf, &'static str)> {
    let mut roots = Vec::new();
    let mut add = |path: Option<PathBuf>, reason: &'static str| {
        if let Some(path) = path.filter(|path| path.is_dir()) {
            roots.push((path, reason));
        }
    };
    add(env_path("ProgramFiles"), "Archivos de programa");
    add(env_path("ProgramFiles(x86)"), "Archivos de programa (x86)");
    add(env_path("ProgramData"), "Datos compartidos (ProgramData)");
    add(env_path("APPDATA"), "Datos de la app (AppData\\Roaming)");
    add(env_path("LOCALAPPDATA"), "Datos de la app (AppData\\Local)");
    add(env_path("LOCALAPPDATA").map(|path| path.join("Programs")), "Programas del usuario");
    add(env_path("USERPROFILE").map(|path| path.join(r"AppData\LocalLow")), "Datos de la app (AppData\\LocalLow)");
    add(env_path("TEMP"), "Archivos temporales");
    roots
}

fn shortcut_roots() -> Vec<(PathBuf, &'static str)> {
    let mut roots = Vec::new();
    let mut add = |path: Option<PathBuf>, reason: &'static str| {
        if let Some(path) = path.filter(|path| path.is_dir()) {
            roots.push((path, reason));
        }
    };
    add(env_path("APPDATA").map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs")), "Menú Inicio");
    add(env_path("ProgramData").map(|p| p.join(r"Microsoft\Windows\Start Menu\Programs")), "Menú Inicio (todos)");
    add(env_path("USERPROFILE").map(|p| p.join("Desktop")), "Escritorio");
    add(env_path("PUBLIC").map(|p| p.join("Desktop")), "Escritorio (todos)");
    roots
}

/// Rutas que nunca se proponen: raíces de búsqueda, Windows, perfiles y unidades.
fn is_protected(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    let windows =
        env_path("SystemRoot").map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_else(|| r"c:\windows".into());
    let is_root = search_roots().iter().chain(shortcut_roots().iter()).any(|(root, _)| root == path);
    let profile = env_path("USERPROFILE").is_some_and(|p| p == path);
    path.components().count() < 3 || is_root || profile || lower.starts_with(&windows)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut counted = 0;
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            // No se siguen enlaces ni uniones: podrían apuntar fuera de la carpeta.
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                total += meta.len();
                counted += 1;
                if counted >= MAX_COUNTED_FILES {
                    return total;
                }
            }
        }
    }
    total
}

struct Scan {
    items: Vec<Leftover>,
}

impl Scan {
    fn push(&mut self, kind: LeftoverKind, path: String, size: u64, reason: &str, recommended: bool) {
        if self.items.iter().any(|item| item.path.eq_ignore_ascii_case(&path)) {
            return;
        }
        self.items.push(Leftover { id: self.items.len(), kind, path, size, reason: reason.to_owned(), recommended });
    }

    fn push_path(&mut self, path: &Path, reason: &str, recommended: bool) {
        if is_protected(path) {
            return;
        }
        let Ok(meta) = fs::symlink_metadata(path) else { return };
        let (kind, size) =
            if meta.is_dir() { (LeftoverKind::Folder, dir_size(path)) } else { (LeftoverKind::File, meta.len()) };
        self.push(kind, path.to_string_lossy().into_owned(), size, reason, recommended);
    }
}

/// Busca restos de una app. Se puede llamar antes o después del desinstalador:
/// lo que el desinstalador borre ya no aparece.
pub fn scan(target: &Target) -> Vec<Leftover> {
    let names = candidate_names(target);
    let company = company_name(&target.publisher);
    let mut scan = Scan { items: Vec::new() };

    if let Some(location) = target.install_location.as_deref().filter(|path| path.exists()) {
        scan.push_path(location, "Carpeta de instalación", true);
    }
    for (root, reason) in search_roots() {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = normalize(&entry.file_name().to_string_lossy());
            if !path.is_dir() {
                continue;
            }
            if names.contains(&name) {
                scan.push_path(&path, reason, true);
            } else if usable(&company) && name == company {
                // Carpeta de la empresa: solo se propone la subcarpeta de esta app.
                let Ok(children) = fs::read_dir(&path) else { continue };
                for child in children.flatten() {
                    if child.path().is_dir() && names.contains(&normalize(&child.file_name().to_string_lossy())) {
                        scan.push_path(&child.path(), reason, true);
                    }
                }
            }
        }
    }
    for (root, reason) in shortcut_roots() {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_shortcut = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"));
            if !is_shortcut && !path.is_dir() {
                continue;
            }
            let stem = normalize(&path.file_stem().unwrap_or_default().to_string_lossy());
            if names.iter().any(|name| stem == *name || (stem.starts_with(name.as_str()) && is_shortcut)) {
                scan.push_path(&path, reason, true);
            }
        }
    }
    for root in [r"HKCU\Software", r"HKLM\SOFTWARE", r"HKLM\SOFTWARE\WOW6432Node"] {
        for key in reg::subkeys(root) {
            let name = normalize(&key);
            if names.contains(&name) {
                scan.push(LeftoverKind::RegistryKey, format!(r"{root}\{key}"), 0, "Configuración en el registro", true);
            } else if usable(&company) && name == company {
                for child in reg::subkeys(&format!(r"{root}\{key}")) {
                    if names.contains(&normalize(&child)) {
                        scan.push(
                            LeftoverKind::RegistryKey,
                            format!(r"{root}\{key}\{child}"),
                            0,
                            "Configuración en el registro",
                            true,
                        );
                    }
                }
            }
        }
    }
    if reg::key_exists(&target.registry_key) {
        scan.push(
            LeftoverKind::RegistryKey,
            target.registry_key.clone(),
            0,
            "Entrada de desinstalación (solo si ya desinstalaste la app)",
            false,
        );
    }
    log::info!(target: "apps", "Restos de «{}»: {} elementos encontrados", target.name, scan.items.len());
    *LAST_SCAN.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = scan.items.clone();
    scan.items
}

/// Manda un archivo o carpeta a la Papelera.
pub(crate) fn recycle(path: &str) -> AppResult<()> {
    let wide: Vec<u16> = path.encode_utf16().chain([0, 0]).collect();
    com::sta(move || {
        let mut operation = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: PCWSTR(wide.as_ptr()),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
            ..Default::default()
        };
        let code = unsafe { SHFileOperationW(&mut operation) };
        if code != 0 || operation.fAnyOperationsAborted.as_bool() {
            // 5 = acceso denegado; 0x78 y similares suelen ser permisos también.
            return Err(if code == 5 || code == 0x78 {
                AppError::NeedsAdmin
            } else {
                AppError::failed(format!("código {code}"))
            });
        }
        Ok(())
    })
}

/// Exporta la clave a un .reg de respaldo y la borra.
fn remove_key(key: &str) -> AppResult<()> {
    let dir = paths::data_dir()?.join("backups").join("registro");
    fs::create_dir_all(&dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(80).collect();
    let file = dir.join(format!("{stamp}-{safe}.reg"));
    let output = process::run_hidden(&system32("reg.exe"), &["export", key, &file.to_string_lossy(), "/y"])?;
    if !output.status.success() {
        return Err(AppError::failed(format!("no se pudo respaldar la clave: {}", process::summary(&output))));
    }
    if reg::is_machine(key) && !crate::util::win::is_elevated() {
        return Err(AppError::NeedsAdmin);
    }
    reg::delete_tree(key)
}

/// Borra los elementos elegidos de la última búsqueda.
pub fn remove(ids: &[usize]) -> Report {
    let items: Vec<Leftover> = {
        let last = LAST_SCAN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        ids.iter().filter_map(|id| last.iter().find(|item| item.id == *id).cloned()).collect()
    };
    let mut report = Report::default();
    for item in items {
        let result = match item.kind {
            LeftoverKind::RegistryKey => remove_key(&item.path),
            _ if is_protected(Path::new(&item.path)) => Err(AppError::invalid("Ruta protegida.")),
            _ => recycle(&item.path),
        };
        match result {
            Ok(()) => {
                report.removed += 1;
                log::info!(target: "apps", "Resto eliminado: {}", item.path);
            }
            Err(err) => {
                report.needs_admin |= matches!(err, AppError::NeedsAdmin);
                log::warn!(target: "apps", "No se pudo eliminar {}: {err}", item.path);
                report.failed.push((item.path, err.to_string()));
            }
        }
    }
    LAST_SCAN.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|item| !ids.contains(&item.id));
    report
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Target, candidate_names, company_name, core_name, is_protected};

    fn target(name: &str, publisher: &str) -> Target {
        Target { name: name.into(), publisher: publisher.into(), install_location: None, registry_key: String::new() }
    }

    #[test]
    fn names_without_versions() {
        assert_eq!(core_name("7-Zip 23.01 (x64)"), "7zip");
        assert_eq!(core_name("Mozilla Firefox (x64 es-AR)"), "mozillafirefox");
        assert_eq!(core_name("Discord"), "discord");
        assert_eq!(company_name("Discord Inc."), "discord");
    }

    #[test]
    fn candidates_skip_generic_names() {
        assert_eq!(candidate_names(&target("Mozilla Firefox", "Mozilla")), vec!["mozillafirefox", "firefox"]);
        assert!(candidate_names(&target("Microsoft", "Microsoft Corporation")).is_empty());
        assert!(candidate_names(&target("Go", "x")).is_empty());
    }

    #[test]
    fn never_proposes_system_or_roots() {
        assert!(is_protected(Path::new(r"C:\")));
        assert!(is_protected(Path::new(r"C:\Windows\System32\drivers")));
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            assert!(is_protected(&PathBuf::from(program_files)));
        }
        assert!(!is_protected(Path::new(r"C:\Program Files\EjemploApp")));
    }
}
