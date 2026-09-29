//! Preferencias de la app (tema, acento, botones de ventana, tamaño de la
//! interfaz, animaciones y Liquid Glass), guardadas en `prefs.json` dentro de la
//! carpeta de datos.

use std::cell::RefCell;
use std::path::PathBuf;

use liquidcontrol_core::{AppError, AppResult, appearance, paths};
use serde::{Deserialize, Serialize};
use slint::{Color, ComponentHandle};

use crate::{App, AppWindow, Theme, chrome};

const FILE_NAME: &str = "prefs.json";

/// Tamaños de la interfaz que se ofrecen (1 = 100 %).
pub const ZOOMS: [f32; 4] = [0.9, 1.0, 1.15, 1.3];
/// Intensidades de animación: ninguna, suaves, normales y exageradas.
pub const MOTIONS: [f32; 4] = [0.0, 0.6, 1.0, 1.8];
/// Máximo del Liquid Glass (150 %).
pub const MAX_GLASS: f32 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowControls {
    #[default]
    Mac,
    Windows,
}

/// Esquinas de la ventana: las de Windows 11 o más redondeadas, como macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowCorners {
    Native,
    #[default]
    Mac,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    pub theme: ThemeMode,
    /// Nombre de un color de macOS o "system" para usar el acento de Windows.
    pub accent: String,
    pub window_controls: WindowControls,
    pub window_corners: WindowCorners,
    /// Tamaño de la interfaz (ver `ZOOMS`).
    pub zoom: f32,
    /// Intensidad de las animaciones (ver `MOTIONS`).
    pub motion: f32,
    /// Intensidad del Liquid Glass (0 = sin vidrio, 1 = normal, 1,5 = exagerado).
    pub glass: f32,
    /// Descargar los íconos de las apps del catálogo desde sus sitios oficiales.
    pub catalog_icons: bool,
    /// Permitir que iCloud descargue las fotos que están solo en la nube para
    /// mostrar su miniatura (sin permiso se usan solo las ya guardadas).
    pub icloud_thumbnails: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            accent: "system".into(),
            window_controls: WindowControls::Mac,
            window_corners: WindowCorners::Mac,
            zoom: 1.0,
            motion: 1.0,
            glass: 1.0,
            catalog_icons: true,
            icloud_thumbnails: false,
        }
    }
}

impl Prefs {
    /// Valores dentro de los rangos permitidos (el archivo lo puede editar cualquiera).
    fn sanitized(mut self) -> Self {
        self.zoom = if self.zoom.is_finite() { self.zoom.clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]) } else { 1.0 };
        self.motion = if self.motion.is_finite() { self.motion.clamp(0.0, MOTIONS[MOTIONS.len() - 1]) } else { 1.0 };
        self.glass = if self.glass.is_finite() { self.glass.clamp(0.0, MAX_GLASS) } else { 1.0 };
        self
    }
}

/// Colores de acento de macOS: (nombre, claro, oscuro). "glass" no es un color
/// plano: marca el acento "Vidrio líquido" (ver `Theme.accent-glass`) y el
/// tono de acá solo se usa como base del degradé y para contextos que
/// necesitan sí o sí un color (por ejemplo el contraste del texto).
pub const ACCENTS: [(&str, u32, u32); 9] = [
    ("blue", 0x007aff, 0x0a84ff),
    ("purple", 0xaf52de, 0xbf5af2),
    ("pink", 0xff2d55, 0xff375f),
    ("red", 0xff3b30, 0xff453a),
    ("orange", 0xff9500, 0xff9f0a),
    ("yellow", 0xffcc00, 0xffd60a),
    ("green", 0x28cd41, 0x30d158),
    ("graphite", 0x8e8e93, 0x98989d),
    ("glass", 0x32ade6, 0x64d2ff),
];

thread_local! {
    static PREFS: RefCell<Prefs> = RefCell::default();
}

fn file() -> AppResult<PathBuf> {
    Ok(paths::data_dir()?.join(FILE_NAME))
}

/// Lee las preferencias (si el archivo falta o está dañado, usa las de fábrica) y las aplica.
pub fn init(app: &AppWindow) {
    let prefs: Prefs = file()
        .and_then(|path| paths::read_json(&path))
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    PREFS.set(prefs.sanitized());
    apply(app);
}

pub fn get() -> Prefs {
    PREFS.with_borrow(Clone::clone)
}

/// Cambia las preferencias, las aplica y las guarda.
pub fn update(app: &AppWindow, change: impl FnOnce(&mut Prefs)) -> AppResult<()> {
    let prefs = PREFS.with_borrow_mut(|prefs| {
        change(prefs);
        *prefs = prefs.clone().sanitized();
        prefs.clone()
    });
    apply(app);
    let json = serde_json::to_string_pretty(&prefs).map_err(AppError::from)?;
    paths::write_atomic(&file()?, &json)
}

/// Aplica tema, acento, estilo de ventana, animaciones, vidrio y tamaño.
/// Se vuelve a llamar si Windows cambia de tema.
pub fn apply(app: &AppWindow) {
    let prefs = get();
    let dark = match prefs.theme {
        ThemeMode::System => appearance::apps_use_dark(),
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
    };
    let accent = accent_color(&prefs.accent, dark);
    let theme = app.global::<Theme>();
    theme.set_dark(dark);
    theme.set_accent(accent);
    theme.set_accent_glass(prefs.accent == "glass");
    theme.set_on_accent(contrast_color(accent));
    theme.set_motion(prefs.motion);
    theme.set_glass(prefs.glass);
    app.global::<App>().set_mac_controls(prefs.window_controls == WindowControls::Mac);
    chrome::apply_zoom(app);
    chrome::apply_corners(app);
}

/// Color de un acento de macOS por nombre; cualquier otro nombre usa el acento de Windows.
pub fn accent_color(name: &str, dark: bool) -> Color {
    match ACCENTS.iter().find(|(id, ..)| *id == name) {
        Some((_, light, dark_rgb)) => {
            let rgb = if dark { *dark_rgb } else { *light };
            Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
        }
        None => {
            let (r, g, b) = appearance::accent_rgb();
            Color::from_rgb_u8(r, g, b)
        }
    }
}

/// Texto blanco o negro según la luminancia del acento (el amarillo lleva texto oscuro).
fn contrast_color(color: Color) -> Color {
    let luminance = 0.299 * f32::from(color.red()) + 0.587 * f32::from(color.green()) + 0.114 * f32::from(color.blue());
    if luminance > 170.0 { Color::from_rgb_u8(0, 0, 0) } else { Color::from_rgb_u8(255, 255, 255) }
}

#[cfg(test)]
mod tests {
    use super::{Prefs, ThemeMode, WindowControls};

    #[test]
    fn tolerates_partial_and_unknown_fields() {
        let prefs: Prefs = serde_json::from_str(r#"{"theme":"dark","extra":1}"#).unwrap();
        assert_eq!(prefs.theme, ThemeMode::Dark);
        assert_eq!(prefs.window_controls, WindowControls::Mac);
        assert_eq!(prefs.accent, "system");
        assert_eq!(prefs.zoom, 1.0);
    }

    #[test]
    fn clamps_out_of_range_values() {
        let prefs: Prefs = serde_json::from_str(r#"{"zoom":9,"motion":-3,"glass":7}"#).unwrap();
        let prefs = prefs.sanitized();
        assert_eq!(prefs.zoom, 1.3);
        assert_eq!(prefs.motion, 0.0);
        assert_eq!(prefs.glass, 1.5);
    }
}
