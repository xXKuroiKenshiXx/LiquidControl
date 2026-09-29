//! Fondo de pantalla con la API oficial `IDesktopWallpaper` (todas las pantallas).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    DESKTOP_WALLPAPER_POSITION, DWPOS_CENTER, DWPOS_FILL, DWPOS_FIT, DWPOS_SPAN, DWPOS_STRETCH, DWPOS_TILE,
    DesktopWallpaper, IDesktopWallpaper,
};
use windows::core::{HSTRING, PCWSTR};

use crate::error::{AppError, AppResult};
use crate::media;
use crate::util::win::system_root;
use crate::util::{com, reg};

const MAX_PER_COLLECTION: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Position {
    Fill,
    Fit,
    Stretch,
    Tile,
    Center,
    Span,
}

impl Position {
    pub const ALL: [Position; 6] = [Self::Fill, Self::Fit, Self::Stretch, Self::Center, Self::Tile, Self::Span];

    pub fn label(self) -> &'static str {
        match self {
            Self::Fill => "Rellenar pantalla",
            Self::Fit => "Ajustar a la pantalla",
            Self::Stretch => "Expandir",
            Self::Center => "Centrar",
            Self::Tile => "Mosaico",
            Self::Span => "Extender en varias pantallas",
        }
    }

    fn to_native(self) -> DESKTOP_WALLPAPER_POSITION {
        match self {
            Self::Fill => DWPOS_FILL,
            Self::Fit => DWPOS_FIT,
            Self::Stretch => DWPOS_STRETCH,
            Self::Tile => DWPOS_TILE,
            Self::Center => DWPOS_CENTER,
            Self::Span => DWPOS_SPAN,
        }
    }

    fn from_native(value: DESKTOP_WALLPAPER_POSITION) -> Self {
        match value {
            DWPOS_FIT => Self::Fit,
            DWPOS_STRETCH => Self::Stretch,
            DWPOS_TILE => Self::Tile,
            DWPOS_CENTER => Self::Center,
            DWPOS_SPAN => Self::Span,
            _ => Self::Fill,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WallpaperState {
    /// `None` = color sólido.
    pub path: Option<PathBuf>,
    pub position: Position,
    pub color: (u8, u8, u8),
}

#[derive(Debug, Clone, Serialize)]
pub struct WallpaperImage {
    pub path: PathBuf,
    pub name: String,
    pub collection: String,
}

fn desktop() -> AppResult<IDesktopWallpaper> {
    Ok(unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL)? })
}

/// Fondo actual: imagen (o color sólido) y ajuste.
pub fn current() -> AppResult<WallpaperState> {
    com::sta(|| unsafe {
        let wallpaper = desktop()?;
        let path = match wallpaper.GetWallpaper(PCWSTR::null()) {
            Ok(raw) => {
                let text = raw.to_string().unwrap_or_default();
                CoTaskMemFree(Some(raw.0 as *const _));
                Some(text)
            }
            // Distinto fondo por pantalla: se usa el del registro.
            Err(_) => reg::read_string(r"HKCU\Control Panel\Desktop", "WallPaper"),
        }
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
        let position = wallpaper.GetPosition().map_or(Position::Fill, Position::from_native);
        let color = wallpaper.GetBackgroundColor().map_or((0, 0, 0), |c| {
            let v = c.0;
            (v as u8, (v >> 8) as u8, (v >> 16) as u8)
        });
        Ok(WallpaperState { path, position, color })
    })
}

fn list_images(dir: &Path, collection: &str, out: &mut Vec<WallpaperImage>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> =
        entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file() && media::is_image(p)).collect();
    files.sort();
    out.extend(files.into_iter().take(MAX_PER_COLLECTION).map(|path| WallpaperImage {
        name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        path,
        collection: collection.to_owned(),
    }));
}

/// Fondos de Windows, de los temas instalados y de la carpeta Imágenes.
pub fn gallery() -> Vec<WallpaperImage> {
    let mut images = Vec::new();
    let web = system_root().join(r"Web\Wallpaper");
    list_images(&web.join("Windows"), "Windows", &mut images);
    if let Ok(entries) = fs::read_dir(&web) {
        let mut themes: Vec<PathBuf> =
            entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect();
        themes.sort();
        for dir in themes.iter().filter(|d| !d.ends_with("Windows")) {
            list_images(dir, "Temas", &mut images);
        }
    }
    if let Some(pictures) = std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Pictures")) {
        list_images(&pictures, "Imágenes", &mut images);
    }
    images
}

pub fn set(path: &Path) -> AppResult<()> {
    log::info!(target: "fondo", "Fondo de pantalla: {}", path.display());
    if !path.is_absolute() || !media::is_image(path) || !path.is_file() {
        return Err(AppError::invalid("Elegí una imagen válida."));
    }
    let path = HSTRING::from(path.as_os_str());
    com::sta(move || unsafe {
        let wallpaper = desktop()?;
        wallpaper.Enable(true)?;
        wallpaper.SetWallpaper(PCWSTR::null(), &path)?;
        Ok(())
    })
}

pub fn set_position(position: Position) -> AppResult<()> {
    log::info!(target: "fondo", "Ajuste del fondo: {}", position.label());
    com::sta(move || unsafe { Ok(desktop()?.SetPosition(position.to_native())?) })
}

/// Color sólido de fondo (sin imagen).
pub fn set_color((r, g, b): (u8, u8, u8)) -> AppResult<()> {
    log::info!(target: "fondo", "Fondo de color sólido #{r:02X}{g:02X}{b:02X}");
    let color = COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16));
    com::sta(move || unsafe {
        let wallpaper = desktop()?;
        wallpaper.SetBackgroundColor(color)?;
        wallpaper.Enable(false)?;
        Ok(())
    })
}
