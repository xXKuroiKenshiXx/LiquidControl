//! iCloud: detecta iCloud para Windows y expone iCloud Drive y Fotos.
//!
//! Por seguridad, LiquidControl nunca maneja credenciales de Apple: el inicio de
//! sesión ocurre en la app oficial. Acá solo se leen las carpetas que la app
//! sincroniza y se abren la app, la Store o iCloud.com.

use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};
use crate::util::{com, shell};
use crate::{actions, leftovers, media};

const PACKAGE_FAMILY: &str = "AppleInc.iCloud_nzyj5cx40ttqa";
const STORE_URI: &str = "ms-windows-store://pdp/?productid=9PKTQ5699M62";
const WEB_URI: &str = "https://www.icloud.com";
const PASSWORDS_WEB_URI: &str = "https://www.icloud.com/passwords/";
const MAX_RECENT: usize = 18;
/// Atributos de archivos que están solo en la nube (Files On-Demand).
const CLOUD_ONLY: u32 = 0x0040_0000 | 0x0004_0000 | 0x0000_1000;

#[derive(Debug, Clone, Serialize)]
pub struct CloudPhoto {
    pub path: PathBuf,
    pub name: String,
    /// Solo está en la nube: no se genera miniatura para no forzar su descarga.
    pub cloud_only: bool,
    /// Cuándo se modificó, en segundos desde 1970 (para buscar por fecha).
    pub modified: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ICloudStatus {
    pub installed: bool,
    pub drive_folder: Option<PathBuf>,
    pub photos_folder: Option<PathBuf>,
    pub recent_photos: Vec<CloudPhoto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Drive,
    Photos,
    App,
    Store,
    Web,
    Passwords,
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

fn installed() -> bool {
    let store_app = std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join("Packages").join(PACKAGE_FAMILY))
        .is_some_and(|p| p.is_dir());
    let classic = std::env::var_os("CommonProgramFiles")
        .map(|base| PathBuf::from(base).join(r"Apple\Internet Services\iCloud.exe"))
        .is_some_and(|p| p.is_file());
    store_app || classic
}

fn drive_folder() -> Option<PathBuf> {
    Some(home().join("iCloudDrive")).filter(|p| p.is_dir())
}

fn photos_folder() -> Option<PathBuf> {
    let base = home().join(r"Pictures\iCloud Photos");
    [base.join("Photos"), base].into_iter().find(|p| p.is_dir())
}

/// Fotos de la carpeta de iCloud, de la más nueva a la más vieja. Leer los
/// atributos no descarga nada de la nube.
fn list_photos(dir: &Path, limit: usize) -> Vec<CloudPhoto> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(SystemTime, PathBuf, bool)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let meta = entry.metadata().ok()?;
            (meta.is_file() && media::is_image(&path)).then(|| {
                // Leer atributos no descarga el archivo desde la nube.
                (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), path, meta.file_attributes() & CLOUD_ONLY != 0)
            })
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.0));
    files
        .into_iter()
        .take(limit)
        .map(|(modified, path, cloud_only)| CloudPhoto {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path,
            cloud_only,
            modified: modified.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |since| since.as_secs()),
        })
        .collect()
}

/// Todas las fotos (hasta `limit`) para el buscador del panel.
pub fn photos(limit: usize) -> Vec<CloudPhoto> {
    photos_folder().as_deref().map(|dir| list_photos(dir, limit)).unwrap_or_default()
}

/// Abre una app del paquete de iCloud (la principal o la de Contraseñas). Requiere MTA.
fn launch_package_app(passwords: bool) -> AppResult<bool> {
    let manager = PackageManager::new()?;
    let packages =
        manager.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(PACKAGE_FAMILY))?;
    let iterator = packages.First()?;
    if !iterator.HasCurrent()? {
        return Ok(false);
    }
    let entries = iterator.Current()?.GetAppListEntries()?;
    for index in 0..entries.Size()? {
        let entry = entries.GetAt(index)?;
        let name = entry.DisplayInfo()?.DisplayName()?.to_string().to_lowercase();
        let is_passwords = name.contains("password") || name.contains("contraseña");
        if is_passwords == passwords {
            return Ok(entry.LaunchAsync()?.get()?);
        }
    }
    Ok(false)
}

pub fn status() -> ICloudStatus {
    let photos = photos_folder();
    ICloudStatus {
        installed: installed(),
        drive_folder: drive_folder(),
        recent_photos: photos.as_deref().map(|dir| list_photos(dir, MAX_RECENT)).unwrap_or_default(),
        photos_folder: photos,
    }
}

pub fn open(target: Target) -> AppResult<()> {
    match target {
        Target::Drive => {
            actions::open_folder(&drive_folder().ok_or_else(|| AppError::not_found("iCloud Drive no está activado."))?)
        }
        Target::Photos => actions::open_folder(
            &photos_folder().ok_or_else(|| AppError::not_found("Fotos de iCloud no está activado."))?,
        ),
        Target::App | Target::Passwords => {
            let passwords = target == Target::Passwords;
            if com::mta(move || launch_package_app(passwords)).unwrap_or(false) {
                return Ok(());
            }
            actions::open_uri(if passwords { PASSWORDS_WEB_URI } else { STORE_URI })
        }
        Target::Store => actions::open_uri(STORE_URI),
        Target::Web => actions::open_uri(WEB_URI),
    }
}

/// Una foto tiene que estar realmente dentro de la carpeta de Fotos de iCloud
/// para poder abrirla o borrarla desde acá (nunca una ruta cualquiera que la
/// interfaz mande por error o algo la haya alterado).
fn in_photos_folder(path: &Path) -> AppResult<PathBuf> {
    let dir = photos_folder().ok_or_else(|| AppError::not_found("Fotos de iCloud no está activado."))?;
    let (Ok(dir), Ok(path)) = (dir.canonicalize(), path.canonicalize()) else {
        return Err(AppError::not_found("La foto ya no está."));
    };
    if !path.starts_with(&dir) || !path.is_file() {
        return Err(AppError::invalid("Esa foto no está en la carpeta de Fotos de iCloud."));
    }
    Ok(path)
}

/// Abre una foto con el visor que tengas configurado por defecto (Fotos de
/// Windows u otro). Si todavía está solo en la nube, se hidrata primero (igual
/// que hace el Explorador al abrir un archivo así): sin esto, algunos
/// visores no la abren la primera vez porque todavía es solo un marcador.
///
/// Si no hay ningún visor asociado a ese tipo de archivo (o falla por
/// cualquier otro motivo), en vez de un simple error se la muestra
/// seleccionada en el Explorador: siempre hay algo útil que pasa al tocarla.
pub fn open_photo(path: &Path) -> AppResult<()> {
    let path = in_photos_folder(path)?;
    let _ = download_photo(&path);
    let target = path.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    let reveal = target.clone();
    com::sta(move || shell::open(&target))
        .or_else(|_| com::sta(move || shell::launch("explorer.exe", &format!("/select,\"{reveal}\""))))
}

/// Manda una foto a la Papelera (se puede deshacer desde ahí, como cualquier
/// borrado con el Explorador).
pub fn delete_photo(path: &Path) -> AppResult<()> {
    let path = in_photos_folder(path)?;
    leftovers::recycle(&path.to_string_lossy())
}

/// Si la foto todavía está solo en la nube, fuerza a iCloud a bajarla entera
/// (leer sus bytes es justo lo que hace que Windows la pida al proveedor de la
/// nube, lo mismo que hace el Explorador al abrirla). Se lee en pedazos para
/// no cargar un video entero en memoria.
pub fn download_photo(path: &Path) -> AppResult<()> {
    use std::io::Read;
    let path = in_photos_folder(path)?;
    let mut file = fs::File::open(&path)?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(err) => return Err(err.into()),
        }
    }
}
