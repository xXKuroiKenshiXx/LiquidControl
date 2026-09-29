//! Información del sistema, almacenamiento, cuenta de usuario e ítems de inicio.

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformation, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PWSTR};

use crate::error::{AppError, AppResult};
use crate::paths;
use crate::util::win::{from_wide, is_elevated};
use crate::util::{process, reg};

// ─────────────────────────── Información ───────────────────────────

const NT_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const CPU_KEY: &str = r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0";
const BIOS_KEY: &str = r"HKLM\HARDWARE\DESCRIPTION\System\BIOS";
/// Textos genéricos que dejan algunos fabricantes en el BIOS.
const OEM_PLACEHOLDERS: &[&str] = &[
    "System manufacturer",
    "System Product Name",
    "To Be Filled By O.E.M.",
    "Default string",
    "Not Applicable",
    "O.E.M.",
];

#[derive(Debug, Clone, Serialize)]
pub struct OsInfo {
    pub name: String,
    pub edition: String,
    pub display_version: String,
    pub build: u32,
    pub ubr: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub name: String,
    pub vram_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub device_name: String,
    pub manufacturer: String,
    pub model: String,
    pub os: OsInfo,
    pub cpu: String,
    pub cpu_cores: u32,
    pub cpu_threads: u32,
    pub ram_total: u64,
    pub ram_available: u64,
    pub gpus: Vec<GpuInfo>,
    pub uptime_secs: u64,
}

fn clean_oem(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty() && !OEM_PLACEHOLDERS.iter().any(|p| v.eq_ignore_ascii_case(p)))
}

fn cpu_counts() -> (u32, u32) {
    let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    let mut len = 0u32;
    unsafe {
        let _ = GetLogicalProcessorInformation(None, &mut len);
    }
    let count = len as usize / size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    if count == 0 {
        return (threads, threads);
    }
    let mut buffer = vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); count];
    let cores = if unsafe { GetLogicalProcessorInformation(Some(buffer.as_mut_ptr()), &mut len) }.is_ok() {
        buffer.iter().filter(|info| info.Relationship == RelationProcessorCore).count() as u32
    } else {
        threads
    };
    (cores.max(1), threads)
}

fn gpus() -> Vec<GpuInfo> {
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut list: Vec<GpuInfo> = Vec::new();
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else { break };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let name = from_wide(&desc.Description);
        if !list.iter().any(|gpu| gpu.name == name) {
            list.push(GpuInfo { name, vram_bytes: desc.DedicatedVideoMemory as u64 });
        }
    }
    list
}

/// Hardware, versión de Windows y tiempo de actividad.
pub fn info() -> AppResult<SystemInfo> {
    let build: u32 = reg::read_string(NT_KEY, "CurrentBuildNumber").and_then(|b| b.parse().ok()).unwrap_or(0);
    let mut name = reg::read_string(NT_KEY, "ProductName").unwrap_or_else(|| "Windows".into());
    // En Windows 11 el registro sigue diciendo "Windows 10": se corrige por número de compilación.
    if build >= 22000 {
        name = name.replacen("Windows 10", "Windows 11", 1);
    }
    let os = OsInfo {
        name,
        edition: reg::read_string(NT_KEY, "EditionID").unwrap_or_default(),
        display_version: reg::read_string(NT_KEY, "DisplayVersion").unwrap_or_default(),
        build,
        ubr: reg::read_dword(NT_KEY, "UBR").unwrap_or(0),
    };
    let manufacturer = clean_oem(reg::read_string(BIOS_KEY, "SystemManufacturer"))
        .or_else(|| clean_oem(reg::read_string(BIOS_KEY, "BaseBoardManufacturer")))
        .unwrap_or_default();
    let model = clean_oem(reg::read_string(BIOS_KEY, "SystemProductName"))
        .or_else(|| clean_oem(reg::read_string(BIOS_KEY, "BaseBoardProduct")))
        .unwrap_or_default();
    let (cpu_cores, cpu_threads) = cpu_counts();

    let mut memory = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut memory)? };

    Ok(SystemInfo {
        device_name: std::env::var("COMPUTERNAME").unwrap_or_default(),
        manufacturer,
        model,
        os,
        cpu: reg::read_string(CPU_KEY, "ProcessorNameString").map(|s| s.trim().to_owned()).unwrap_or_default(),
        cpu_cores,
        cpu_threads,
        ram_total: memory.ullTotalPhys,
        ram_available: memory.ullAvailPhys,
        gpus: gpus(),
        uptime_secs: unsafe { GetTickCount64() } / 1000,
    })
}

// ─────────────────────────── Almacenamiento ───────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DriveKind {
    Fixed,
    Removable,
    CdRom,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct Drive {
    pub root: String,
    pub label: String,
    pub file_system: String,
    pub total: u64,
    pub free: u64,
    pub kind: DriveKind,
}

/// Unidades locales con su espacio libre (las de red se omiten: pueden tardar segundos).
pub fn storage() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let root = format!("{}:\\", (b'A' + i) as char);
            let root_w = HSTRING::from(root.as_str());
            let kind = match unsafe { GetDriveTypeW(&root_w) } {
                2 => DriveKind::Removable,
                3 => DriveKind::Fixed,
                4 => return None,
                5 => DriveKind::CdRom,
                _ => DriveKind::Other,
            };
            let mut label = [0u16; 261];
            let mut file_system = [0u16; 261];
            unsafe { GetVolumeInformationW(&root_w, Some(&mut label), None, None, None, Some(&mut file_system)) }
                .ok()?;
            let (mut free, mut total) = (0u64, 0u64);
            unsafe { GetDiskFreeSpaceExW(&root_w, Some(&mut free), Some(&mut total), None) }.ok()?;
            Some(Drive { root, label: from_wide(&label), file_system: from_wide(&file_system), total, free, kind })
        })
        .collect()
}

// ─────────────────────────── Cuenta ───────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub user_name: String,
    pub display_name: String,
    /// Imagen de la cuenta (JPG/PNG en disco). La interfaz genera la miniatura.
    pub picture_path: Option<PathBuf>,
    pub microsoft_account: bool,
    pub email: Option<String>,
}

fn display_name() -> Option<String> {
    let mut len = 256u32;
    let mut buffer = vec![0u16; len as usize];
    let ok = unsafe { GetUserNameExW(NameDisplay, Some(PWSTR(buffer.as_mut_ptr())), &mut len) };
    ok.then(|| from_wide(&buffer)).filter(|name| !name.trim().is_empty())
}

/// SID del usuario actual en texto ("S-1-5-21-...").
fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        // Búfer de u64 para respetar la alineación de TOKEN_USER.
        let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
        let ok = GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr() as *mut c_void), len, &mut len).is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return None;
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut sid = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut sid).ok()?;
        let text = sid.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(sid.0 as *mut c_void)));
        text
    }
}

fn account_picture() -> Option<PathBuf> {
    let sid = current_user_sid()?;
    let key = format!(r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users\{sid}");
    ["Image192", "Image208", "Image240", "Image96"]
        .iter()
        .find_map(|name| reg::read_string(&key, name))
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

/// Usuario actual, foto y si usa una cuenta Microsoft.
pub fn account() -> Account {
    let user_name = std::env::var("USERNAME").unwrap_or_default();
    let email = reg::subkeys(r"HKCU\Software\Microsoft\IdentityCRL\UserExtendedProperties")
        .into_iter()
        .find(|key| key.contains('@'));
    Account {
        display_name: display_name().unwrap_or_else(|| user_name.clone()),
        user_name,
        picture_path: account_picture(),
        microsoft_account: email.is_some(),
        email,
    }
}

// ─────────────────────────── Ítems de inicio ───────────────────────────

const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupSource {
    User,
    Machine,
    Machine32,
    FolderUser,
    FolderCommon,
}

impl StartupSource {
    const ALL: [StartupSource; 5] = [Self::User, Self::Machine, Self::Machine32, Self::FolderUser, Self::FolderCommon];

    fn id(self) -> &'static str {
        match self {
            Self::User => "hkcu",
            Self::Machine => "hklm",
            Self::Machine32 => "hklm32",
            Self::FolderUser => "folderUser",
            Self::FolderCommon => "folderCommon",
        }
    }

    /// Descripción para la interfaz.
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "Tu usuario",
            Self::Machine => "Todos los usuarios",
            Self::Machine32 => "Todos los usuarios (32 bits)",
            Self::FolderUser => "Carpeta Inicio",
            Self::FolderCommon => "Carpeta Inicio común",
        }
    }

    pub fn needs_admin(self) -> bool {
        matches!(self, Self::Machine | Self::Machine32 | Self::FolderCommon)
    }

    fn run_key(self) -> Option<&'static str> {
        match self {
            Self::User => Some(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run"),
            Self::Machine => Some(r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run"),
            Self::Machine32 => Some(r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run"),
            _ => None,
        }
    }

    fn folder(self) -> Option<PathBuf> {
        let (var, tail) = match self {
            Self::FolderUser => ("APPDATA", r"Microsoft\Windows\Start Menu\Programs\Startup"),
            Self::FolderCommon => ("ProgramData", r"Microsoft\Windows\Start Menu\Programs\StartUp"),
            _ => return None,
        };
        std::env::var_os(var).map(|base| PathBuf::from(base).join(tail))
    }

    fn approved_key(self) -> String {
        match self {
            Self::User => format!(r"HKCU\{APPROVED}\Run"),
            Self::Machine => format!(r"HKLM\{APPROVED}\Run"),
            Self::Machine32 => format!(r"HKLM\{APPROVED}\Run32"),
            Self::FolderUser => format!(r"HKCU\{APPROVED}\StartupFolder"),
            Self::FolderCommon => format!(r"HKLM\{APPROVED}\StartupFolder"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StartupItem {
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: StartupSource,
    pub enabled: bool,
    /// Ejecutable o acceso directo (para mostrar su ícono), si existe.
    pub exe: Option<PathBuf>,
}

/// En StartupApproved el primer byte par significa habilitado; si no hay valor, también.
fn is_approved(approved_key: &str, name: &str) -> bool {
    reg::read_binary(approved_key, name).and_then(|bytes| bytes.first().copied()).is_none_or(|flag| flag & 1 == 0)
}

/// Programas que se abren al iniciar sesión (registro y carpetas Inicio).
pub fn startup_items() -> Vec<StartupItem> {
    let mut items = Vec::new();
    for source in StartupSource::ALL {
        let mut entries: Vec<(String, String)> = Vec::new();
        if let Some(key) = source.run_key().and_then(reg::open_read) {
            for (name, _) in key.enum_values().filter_map(Result::ok) {
                if let Ok(command) = key.get_value::<String, _>(&name) {
                    entries.push((name, command));
                }
            }
        }
        if let Some(dir) = source.folder().and_then(|folder| fs::read_dir(folder).ok()) {
            for entry in dir.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.eq_ignore_ascii_case("desktop.ini") {
                    entries.push((name, entry.path().to_string_lossy().into_owned()));
                }
            }
        }
        let approved = source.approved_key();
        for (name, command) in entries {
            let exe =
                if source.folder().is_some() { Some(PathBuf::from(&command)) } else { process::command_path(&command) };
            items.push(StartupItem {
                id: format!("{}:{name}", source.id()),
                enabled: is_approved(&approved, &name),
                name: name.trim_end_matches(".lnk").to_owned(),
                exe,
                command,
                source,
            });
        }
    }
    items.sort_by_key(|item| item.name.to_lowercase());
    items
}

fn filetime_now() -> u64 {
    // FILETIME: intervalos de 100 ns desde 1601-01-01.
    const EPOCH_DIFF: u64 = 116_444_736_000_000_000;
    let since_unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    EPOCH_DIFF + since_unix.as_nanos() as u64 / 100
}

/// Activa o desactiva un ítem de inicio (igual que el Administrador de tareas).
pub fn set_startup_item(id: &str, enabled: bool) -> AppResult<()> {
    let (source_id, name) = id.split_once(':').ok_or_else(|| AppError::invalid("Identificador inválido."))?;
    let source = StartupSource::ALL
        .into_iter()
        .find(|s| s.id() == source_id)
        .ok_or_else(|| AppError::invalid("Origen desconocido."))?;
    // Solo se permite tocar ítems que existen de verdad.
    if !startup_items().iter().any(|item| item.id == id) {
        return Err(AppError::not_found("Ese ítem de inicio ya no existe."));
    }
    if source.needs_admin() && !is_elevated() {
        return Err(AppError::NeedsAdmin);
    }
    let mut value = vec![if enabled { 0x02 } else { 0x03 }, 0, 0, 0];
    value.extend_from_slice(&if enabled { 0u64 } else { filetime_now() }.to_le_bytes());
    reg::write_binary(&source.approved_key(), name, &value)?;
    log::info!(target: "inicio", "«{name}» {} al iniciar sesión", if enabled { "se abre" } else { "ya no se abre" });
    Ok(())
}

// ─────────────────────────── Ítems de inicio quitados ───────────────────────────

/// Ítem quitado del inicio, guardado para poder restaurarlo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovedStartup {
    pub source: StartupSource,
    /// Nombre del valor del registro o del archivo del acceso directo.
    pub name: String,
    /// Línea de comandos (registro) o ruta original del acceso directo (carpeta).
    pub command: String,
    /// Copia del acceso directo dentro de la carpeta de datos.
    #[serde(default)]
    pub backup: Option<PathBuf>,
}

fn removed_file() -> AppResult<PathBuf> {
    Ok(paths::data_dir()?.join("startup-removed.json"))
}

/// Ítems quitados que se pueden restaurar (el más reciente al final).
pub fn removed_startup_items() -> Vec<RemovedStartup> {
    removed_file()
        .ok()
        .and_then(|file| fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(paths::strip_bom(&text)).ok())
        .unwrap_or_default()
}

fn save_removed(list: &[RemovedStartup]) -> AppResult<()> {
    paths::write_atomic(&removed_file()?, &serde_json::to_string_pretty(list)?)
}

/// Nombre simple de archivo (sin carpetas ni "..").
fn plain_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['\\', '/', ':']) && name != "." && name != ".."
}

/// Copias de los ítems "para todos los usuarios": se guardan en HKLM, donde solo
/// escriben los administradores, y no en la carpeta de datos, que cualquier
/// programa del usuario puede modificar. Así nadie puede cambiar lo que después
/// se restaura con permisos de administrador.
const PROTECTED_BACKUP: &str = r"HKLM\SOFTWARE\LiquidControl\StartupRemoved";
/// Tamaño máximo de un acceso directo guardado en el registro.
const MAX_SHORTCUT_BYTES: usize = 256 * 1024;

fn protected_key(source: StartupSource) -> String {
    let leaf = match source {
        StartupSource::Machine32 => "Run32",
        StartupSource::FolderCommon => "StartupFolder",
        _ => "Run",
    };
    format!(r"{PROTECTED_BACKUP}\{leaf}")
}

fn user_backup_dir() -> AppResult<PathBuf> {
    Ok(paths::data_dir()?.join("startup-removed"))
}

/// La copia de un acceso directo del usuario tiene que estar directamente en su carpeta de copias.
fn is_user_backup(path: &Path, dir: &Path) -> bool {
    path.parent() == Some(dir) && path.file_name().and_then(|name| name.to_str()).is_some_and(plain_file_name)
}

/// Quita un ítem de inicio (no solo lo desactiva). Queda una copia para restaurarlo.
pub fn remove_startup_item(id: &str) -> AppResult<()> {
    let item = startup_items()
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| AppError::not_found("Ese ítem de inicio ya no existe."))?;
    let protected = item.source.needs_admin();
    if protected && !is_elevated() {
        return Err(AppError::NeedsAdmin);
    }
    let (_, name) = id.split_once(':').ok_or_else(|| AppError::invalid("Identificador inválido."))?;
    let mut removed = removed_startup_items();
    let backup = match item.source.run_key() {
        Some(run_key) => {
            if protected {
                reg::write_string(&protected_key(item.source), name, &item.command)?;
            }
            reg::delete_value(run_key, name)?;
            None
        }
        None => {
            if !plain_file_name(name) {
                return Err(AppError::invalid("Nombre de acceso directo inválido."));
            }
            let original = PathBuf::from(&item.command);
            if protected {
                let bytes = fs::read(&original)?;
                if bytes.len() > MAX_SHORTCUT_BYTES {
                    return Err(AppError::invalid("El acceso directo es demasiado grande."));
                }
                reg::write_binary(&protected_key(item.source), name, &bytes)?;
                fs::remove_file(&original)?;
                None
            } else {
                // El acceso directo se mueve a la carpeta de datos en lugar de borrarse.
                let dir = user_backup_dir()?;
                fs::create_dir_all(&dir)?;
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
                let backup = dir.join(format!("{stamp}-{name}"));
                fs::copy(&original, &backup)?;
                fs::remove_file(&original)?;
                Some(backup)
            }
        }
    };
    removed.push(RemovedStartup { source: item.source, name: name.to_owned(), command: item.command.clone(), backup });
    let _ = reg::delete_value(&item.source.approved_key(), name);
    save_removed(&removed)?;
    log::info!(target: "inicio", "Se quitó «{}» del inicio ({})", item.name, item.source.label());
    Ok(())
}

/// Vuelve a agregar un ítem quitado (por su posición en `removed_startup_items`).
pub fn restore_startup_item(index: usize) -> AppResult<()> {
    let mut removed = removed_startup_items();
    let entry = removed.get(index).cloned().ok_or_else(|| AppError::not_found("Ese ítem ya no está en la lista."))?;
    let protected = entry.source.needs_admin();
    if protected && !is_elevated() {
        return Err(AppError::NeedsAdmin);
    }
    let missing = || AppError::not_found("No se encontró la copia para restaurar.");
    match entry.source.run_key() {
        // La línea de comandos sale de la copia protegida, nunca del archivo de la carpeta de datos.
        Some(run_key) if protected => {
            let key = protected_key(entry.source);
            let command = reg::read_string(&key, &entry.name).ok_or_else(missing)?;
            reg::write_string(run_key, &entry.name, &command)?;
            let _ = reg::delete_value(&key, &entry.name);
        }
        Some(run_key) => reg::write_string(run_key, &entry.name, &entry.command)?,
        None => {
            if !plain_file_name(&entry.name) {
                return Err(AppError::invalid("Nombre de acceso directo inválido."));
            }
            let folder = entry.source.folder().ok_or_else(|| AppError::failed("Carpeta Inicio desconocida."))?;
            let target = folder.join(&entry.name);
            if protected {
                let key = protected_key(entry.source);
                let bytes = reg::read_binary(&key, &entry.name).ok_or_else(missing)?;
                fs::write(&target, &bytes)?;
                let _ = reg::delete_value(&key, &entry.name);
            } else {
                let dir = user_backup_dir()?;
                let backup = entry.backup.as_deref().filter(|path| is_user_backup(path, &dir)).ok_or_else(missing)?;
                fs::copy(backup, &target)?;
                let _ = fs::remove_file(backup);
            }
        }
    }
    removed.remove(index);
    save_removed(&removed)?;
    log::info!(target: "inicio", "Se restauró «{}» al inicio", entry.name.trim_end_matches(".lnk"));
    Ok(())
}

/// Le pide a Windows Update que busque actualizaciones ahora mismo (la
/// misma acción que el botón "Buscar actualizaciones" de Configuración),
/// vía la API COM oficial de Windows Update. Solo dispara la búsqueda: el
/// progreso y la instalación siguen viéndose en Configuración de Windows.
pub fn check_for_updates() -> AppResult<()> {
    const SCRIPT: &str = "(New-Object -ComObject Microsoft.Update.AutoUpdate).DetectNow()";
    process::powershell("buscar actualizaciones", SCRIPT)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{StartupSource, is_user_backup, plain_file_name, protected_key};

    #[test]
    fn machine_backups_live_in_hklm() {
        assert_eq!(protected_key(StartupSource::Machine), r"HKLM\SOFTWARE\LiquidControl\StartupRemoved\Run");
        assert_eq!(protected_key(StartupSource::Machine32), r"HKLM\SOFTWARE\LiquidControl\StartupRemoved\Run32");
        assert_eq!(
            protected_key(StartupSource::FolderCommon),
            r"HKLM\SOFTWARE\LiquidControl\StartupRemoved\StartupFolder"
        );
    }

    #[test]
    fn user_backups_must_stay_in_their_folder() {
        let dir = Path::new(r"C:\datos\startup-removed");
        assert!(is_user_backup(&dir.join("1700000000-App.lnk"), dir));
        assert!(!is_user_backup(Path::new(r"C:\Windows\System32\evil.lnk"), dir));
        assert!(!is_user_backup(&dir.join("sub").join("x.lnk"), dir));
        assert!(!plain_file_name(".."));
        assert!(!plain_file_name(r"..\x.lnk"));
    }
}
