//! Aplicaciones: programas instalados, desinstalación, limpieza de apps
//! preinstaladas (lista cerrada del catálogo), winget y puntos de restauración.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};
use crate::util::win::{is_elevated, require_admin, system32};
use crate::util::{com, process, reg, shell};

const APPX_CATALOG_JSON: &str = include_str!("../catalog/appx.json");
const WINGET_CATALOG_JSON: &str = include_str!("../catalog/winget.json");
const UNINSTALL_ROOTS: &[(&str, &str)] = &[
    ("hklm", r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    ("hklm32", r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    ("hkcu", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall"),
];

/// Script fijo: crea un punto de restauración aunque ya se haya creado uno en
/// las últimas 24 h, y deja la frecuencia del sistema como estaba.
const RESTORE_POINT_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$key = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore'
$previous = (Get-ItemProperty -Path $key -Name SystemRestorePointCreationFrequency -ErrorAction SilentlyContinue).SystemRestorePointCreationFrequency
Set-ItemProperty -Path $key -Name SystemRestorePointCreationFrequency -Value 0 -Type DWord
try {
  Checkpoint-Computer -Description 'LiquidControl' -RestorePointType 'MODIFY_SETTINGS'
} finally {
  if ($null -eq $previous) {
    Remove-ItemProperty -Path $key -Name SystemRestorePointCreationFrequency -ErrorAction SilentlyContinue
  } else {
    Set-ItemProperty -Path $key -Name SystemRestorePointCreationFrequency -Value $previous -Type DWord
  }
}
"#;

// ─────────────────────────── Programas instalados ───────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub size_kb: u64,
    /// "AAAA-MM-DD".
    pub install_date: Option<String>,
    pub can_uninstall: bool,
    /// Ícono que declara el instalador (ejecutable o .ico), si existe.
    pub icon: Option<PathBuf>,
    /// Carpeta de instalación que declara el instalador, si existe.
    pub install_location: Option<PathBuf>,
    /// Clave del registro con la entrada de desinstalación.
    pub registry_key: String,
}

/// Carpeta de `InstallLocation` si es una ruta absoluta que existe.
fn install_location(raw: &str) -> Option<PathBuf> {
    let path = PathBuf::from(process::expand_env(raw.trim().trim_matches('"').trim_end_matches('\\')));
    (path.is_absolute() && path.is_dir()).then_some(path)
}

/// Clave del registro de una app a partir de su id ("hklm:{clave}").
fn registry_key(id: &str) -> AppResult<String> {
    let (prefix, key) = id.split_once(':').ok_or_else(|| AppError::invalid("Identificador inválido."))?;
    let root = UNINSTALL_ROOTS
        .iter()
        .find(|(p, _)| *p == prefix)
        .map(|(_, root)| *root)
        .ok_or_else(|| AppError::invalid("Identificador inválido."))?;
    if key.is_empty() || key.contains('\\') {
        return Err(AppError::invalid("Identificador inválido."));
    }
    Ok(format!(r"{root}\{key}"))
}

/// Si la app sigue instalada (su entrada de desinstalación existe).
pub fn is_installed(id: &str) -> bool {
    registry_key(id).is_ok_and(|key| reg::key_exists(&key))
}

fn format_date(raw: &str) -> Option<String> {
    (raw.len() == 8 && raw.chars().all(|c| c.is_ascii_digit()))
        .then(|| format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..8]))
}

/// Ruta del ícono a partir de `DisplayIcon` ("C:\App\app.exe,0" o "\"C:\App\app.ico\"").
fn icon_path(raw: &str) -> Option<PathBuf> {
    let raw = raw.trim().trim_matches('"');
    let path = match raw.rsplit_once(',') {
        Some((path, index)) if index.trim().parse::<i32>().is_ok() => path.trim().trim_matches('"'),
        _ => raw,
    };
    let path = PathBuf::from(process::expand_env(path));
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// Programas instalados (como "Programas y características"), sin componentes ni parches.
pub fn installed() -> Vec<InstalledApp> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for (prefix, root) in UNINSTALL_ROOTS {
        for key in reg::subkeys(root) {
            let path = format!(r"{root}\{key}");
            let Some(name) =
                reg::read_string(&path, "DisplayName").map(|n| n.trim().to_owned()).filter(|n| !n.is_empty())
            else {
                continue;
            };
            let hidden = reg::read_dword(&path, "SystemComponent") == Some(1)
                || reg::read_string(&path, "ParentKeyName").is_some()
                || matches!(
                    reg::read_string(&path, "ReleaseType").as_deref(),
                    Some("Update" | "Hotfix" | "Security Update")
                );
            if hidden {
                continue;
            }
            let version = reg::read_string(&path, "DisplayVersion").unwrap_or_default();
            if !seen.insert((name.to_lowercase(), version.clone())) {
                continue;
            }
            apps.push(InstalledApp {
                id: format!("{prefix}:{key}"),
                publisher: reg::read_string(&path, "Publisher").unwrap_or_default().trim().to_owned(),
                size_kb: reg::read_dword(&path, "EstimatedSize").unwrap_or(0).into(),
                install_date: reg::read_string(&path, "InstallDate").and_then(|d| format_date(&d)),
                can_uninstall: reg::read_string(&path, "UninstallString").is_some()
                    && reg::read_dword(&path, "NoRemove") != Some(1),
                icon: reg::read_string(&path, "DisplayIcon").and_then(|raw| icon_path(&raw)),
                install_location: reg::read_string(&path, "InstallLocation").and_then(|raw| install_location(&raw)),
                registry_key: path.clone(),
                name,
                version,
            });
        }
    }
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

fn msi_product_code(command: &str) -> Option<String> {
    let start = command.find('{')?;
    let end = command[start..].find('}')? + start;
    let code = &command[start..=end];
    (code.len() == 38 && code[1..37].chars().all(|c| c.is_ascii_hexdigit() || c == '-')).then(|| code.to_owned())
}

fn uninstall_command(id: &str) -> AppResult<(String, String)> {
    let command = reg::read_string(&registry_key(id)?, "UninstallString")
        .ok_or_else(|| AppError::not_found("Esta app no tiene desinstalador."))?;
    let (exe, args) = process::split_command(&command);
    // Instaladores MSI: se usa el msiexec del sistema con el código de producto,
    // en lugar de ejecutar la cadena del registro tal cual.
    if exe.to_ascii_lowercase().ends_with("msiexec.exe") || exe.eq_ignore_ascii_case("msiexec") {
        let code = msi_product_code(&command).ok_or_else(|| AppError::failed("Desinstalador MSI inválido."))?;
        return Ok((system32("msiexec.exe").to_string_lossy().into_owned(), format!("/x {code}")));
    }
    Ok((exe, args))
}

/// Abre el desinstalador de una app (si pide administrador, Windows muestra el UAC).
pub fn uninstall(id: &str) -> AppResult<()> {
    // Las apps instaladas solo para el usuario guardan su desinstalador en HKCU, que
    // cualquier programa del usuario puede modificar: nunca se ejecuta con permisos elevados.
    if id.starts_with("hkcu:") && is_elevated() {
        return Err(AppError::invalid(
            "Por seguridad, esta app se desinstala sin permisos de administrador: abrí LiquidControl normalmente.",
        ));
    }
    let (exe, args) = uninstall_command(id)?;
    com::sta(move || shell::launch(&exe, &args))
}

// ─────────────────────────── Apps preinstaladas (AppX) ───────────────────────────

#[derive(Deserialize)]
struct AppxCatalog {
    packages: Vec<AppxEntry>,
}

#[derive(Deserialize, Clone)]
struct AppxEntry {
    name: String,
    label: String,
}

fn appx_catalog() -> AppResult<&'static [AppxEntry]> {
    static CATALOG: OnceLock<Result<AppxCatalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| serde_json::from_str(APPX_CATALOG_JSON).map_err(|e| e.to_string()))
        .as_ref()
        .map(|c| c.packages.as_slice())
        .map_err(|e| AppError::failed(format!("Catálogo de apps inválido: {e}")))
}

#[derive(Debug, Clone, Serialize)]
pub struct AppxPackage {
    pub name: String,
    pub label: String,
    pub installed: bool,
}

/// Paquetes del usuario actual: (nombre, nombre completo). Requiere MTA.
fn user_packages() -> AppResult<Vec<(String, String)>> {
    let manager = PackageManager::new()?;
    let packages = manager.FindPackagesByUserSecurityId(&HSTRING::new())?;
    let iterator = packages.First()?;
    let mut list = Vec::new();
    while iterator.HasCurrent()? {
        let id = iterator.Current()?.Id()?;
        list.push((id.Name()?.to_string(), id.FullName()?.to_string()));
        iterator.MoveNext()?;
    }
    Ok(list)
}

/// Apps preinstaladas del catálogo y si están instaladas para este usuario.
pub fn appx_list() -> AppResult<Vec<AppxPackage>> {
    com::mta(|| {
        let installed: HashSet<String> = user_packages()?.into_iter().map(|(name, _)| name).collect();
        Ok(appx_catalog()?
            .iter()
            .map(|entry| AppxPackage {
                installed: installed.contains(&entry.name),
                name: entry.name.clone(),
                label: entry.label.clone(),
            })
            .collect())
    })
}

/// Quita apps preinstaladas (solo del catálogo) para el usuario actual. Devuelve las quitadas.
pub fn appx_remove(names: &[String]) -> AppResult<Vec<String>> {
    let names = names.to_vec();
    com::mta(move || {
        let allowed: HashSet<&str> = appx_catalog()?.iter().map(|e| e.name.as_str()).collect();
        if let Some(bad) = names.iter().find(|n| !allowed.contains(n.as_str())) {
            return Err(AppError::invalid(format!("{bad} no está en la lista de apps removibles.")));
        }
        let manager = PackageManager::new()?;
        let mut removed = Vec::new();
        for (name, full_name) in user_packages()? {
            if !names.contains(&name) {
                continue;
            }
            let ok = manager
                .RemovePackageAsync(&HSTRING::from(full_name.as_str()))
                .and_then(|op| op.get())
                .is_ok_and(|result| result.ExtendedErrorCode().is_ok_and(|code| code.is_ok()));
            if ok && !removed.contains(&name) {
                log::info!(target: "apps", "Se quitó el paquete {name}");
                removed.push(name);
            }
        }
        Ok(removed)
    })
}

// ─────────────────────────── winget ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct WingetApp {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Sitio oficial (solo el host) de donde se toma el ícono público de la app.
    #[serde(default)]
    pub site: String,
    /// Origen de winget: "winget" (repositorio comunitario) o "msstore" (Microsoft Store).
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "winget".to_owned()
}

#[derive(Debug, Deserialize)]
pub struct WingetGroup {
    pub title: String,
    pub apps: Vec<WingetApp>,
}

#[derive(Deserialize)]
struct WingetCatalog {
    groups: Vec<WingetGroup>,
}

/// Catálogo curado de apps para instalar con winget.
pub fn winget_catalog() -> AppResult<&'static [WingetGroup]> {
    static CATALOG: OnceLock<Result<WingetCatalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| serde_json::from_str(WINGET_CATALOG_JSON).map_err(|e| e.to_string()))
        .as_ref()
        .map(|c| c.groups.as_slice())
        .map_err(|e| AppError::failed(format!("Catálogo de winget inválido: {e}")))
}

/// Ruta de winget. El alias vive en una carpeta que cualquier programa del usuario
/// puede modificar, así que desde una sesión elevada no se usa.
fn winget() -> AppResult<PathBuf> {
    if is_elevated() {
        return Err(AppError::invalid(
            "Por seguridad, las apps se instalan sin permisos de administrador: abrí LiquidControl normalmente.",
        ));
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join(r"Microsoft\WindowsApps\winget.exe"))
        .filter(|path| path.exists())
        .ok_or_else(|| {
            AppError::not_found("No se encontró winget: instalá «Instalador de aplicación» desde la Microsoft Store.")
        })
}

fn valid_package_id(id: &str) -> bool {
    (2..=128).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || ".-_+".contains(c))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WingetExport {
    #[serde(default)]
    sources: Vec<WingetSource>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WingetSource {
    #[serde(default)]
    packages: Vec<WingetPackage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WingetPackage {
    package_identifier: String,
}

/// IDs de paquetes instalados (usa `export`, que es JSON y no depende del idioma).
pub fn winget_installed() -> AppResult<Vec<String>> {
    let file = std::env::temp_dir().join(format!("liquidcontrol-winget-{}.json", std::process::id()));
    let path = file.to_string_lossy().into_owned();
    let _ = process::run_hidden(&winget()?, &["export", "-o", &path, "--disable-interactivity"])?;
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    let export: WingetExport = serde_json::from_str(&text).unwrap_or(WingetExport { sources: Vec::new() });
    Ok(export.sources.into_iter().flat_map(|s| s.packages).map(|p| p.package_identifier).collect())
}

/// Instala una app del catálogo (tarda lo que tarde la descarga).
pub fn winget_install(id: &str) -> AppResult<()> {
    let app = winget_catalog()?
        .iter()
        .flat_map(|group| &group.apps)
        .find(|app| app.id == id)
        .ok_or_else(|| AppError::invalid("Esa app no está en el catálogo."))?;
    if !valid_package_id(&app.id) {
        return Err(AppError::invalid("Identificador de paquete inválido."));
    }
    let output = process::run_hidden(
        &winget()?,
        &[
            "install",
            "--id",
            &app.id,
            "--exact",
            "--source",
            &app.source,
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ],
    )?;
    if output.status.success() {
        log::info!(target: "apps", "Se instaló {} ({})", app.name, app.id);
        Ok(())
    } else {
        Err(AppError::failed(format!("winget no pudo instalar {}: {}", app.name, process::summary(&output))))
    }
}

/// Crea un punto de restauración del sistema (requiere administrador).
pub fn create_restore_point() -> AppResult<()> {
    require_admin()?;
    process::powershell("crear un punto de restauración", RESTORE_POINT_SCRIPT).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_msi_codes_and_icons() {
        assert_eq!(
            msi_product_code("MsiExec.exe /X{12345678-1234-1234-1234-123456789ABC}").as_deref(),
            Some("{12345678-1234-1234-1234-123456789ABC}")
        );
        assert_eq!(icon_path(r"C:\no\existe.exe,0"), None);
        let notepad = system32("notepad.exe");
        assert_eq!(icon_path(&format!("\"{}\",0", notepad.display())), Some(notepad.clone()));
        assert_eq!(icon_path(&notepad.display().to_string()), Some(notepad));
    }

    #[test]
    fn validates_winget_ids() {
        assert!(valid_package_id("Notepad++.Notepad++"));
        assert!(valid_package_id("Microsoft.VisualStudioCode"));
        assert!(valid_package_id("9PFXXSHC64H3"));
        assert!(!valid_package_id("foo; rm -rf"));
        assert!(!valid_package_id("--help"));
    }

    #[test]
    fn catalogs_parse() {
        assert!(!appx_catalog().expect("catálogo appx").is_empty());
        let groups = winget_catalog().expect("catálogo winget");
        for app in groups.iter().flat_map(|g| &g.apps) {
            assert!(valid_package_id(&app.id), "id inválido: {}", app.id);
            assert!(["winget", "msstore"].contains(&app.source.as_str()), "origen inválido: {}", app.id);
            assert!(
                app.site.is_empty()
                    || (app.site.contains('.')
                        && app.site.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')),
                "sitio inválido: {}",
                app.site
            );
        }
    }
}
